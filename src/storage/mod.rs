use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::Mutex};

use crate::{
    events::{EventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink},
    model::ModelModality,
};

mod input;
mod message_log;
mod trajectory;

pub const MESSAGE_FORMAT: &str = "fiasco-message";
const RUN_RECORD_VERSION: u32 = 10;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    /// A reusable delegated agent is waiting for more input.
    Idle,
    Completed,
    Failed,
    Cancelled,
    /// A reusable delegated agent was explicitly closed.
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub version: u32,
    pub id: String,
    pub parent_run_id: Option<String>,
    pub state: RunState,
    pub prompt: String,
    /// Stable agent capability profile used to rebuild the same run on resume.
    pub profile: String,
    pub depth: usize,
    /// Delegation capacity frozen when this run is created. The delegate
    /// schema remains present at zero; execution then returns a local error.
    pub remaining_delegation_depth: usize,
    pub additional_instructions: Option<String>,
    pub tool_schema_sha256: String,
    pub provider: String,
    /// Non-secret identity of provider settings that affect wire compatibility.
    pub provider_resume_fingerprint: String,
    pub model: String,
    pub model_modalities: BTreeSet<ModelModality>,
    pub message_format: String,
    pub cwd: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RunRecord {
    pub fn new(
        id: impl Into<String>,
        prompt: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        cwd: PathBuf,
        parent_run_id: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            version: RUN_RECORD_VERSION,
            id: id.into(),
            parent_run_id,
            state: RunState::Queued,
            prompt: prompt.into(),
            profile: "root".to_owned(),
            depth: 0,
            remaining_delegation_depth: 0,
            additional_instructions: None,
            tool_schema_sha256: String::new(),
            provider: provider.into(),
            provider_resume_fingerprint: String::new(),
            model: model.into(),
            model_modalities: BTreeSet::from([ModelModality::Text]),
            message_format: MESSAGE_FORMAT.to_owned(),
            cwd,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_execution_context(
        mut self,
        profile: impl Into<String>,
        depth: usize,
        additional_instructions: Option<String>,
        remaining_delegation_depth: usize,
    ) -> Self {
        self.profile = profile.into();
        self.depth = depth;
        self.additional_instructions = additional_instructions;
        self.remaining_delegation_depth = remaining_delegation_depth;
        self
    }

    pub fn with_provider_resume_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.provider_resume_fingerprint = fingerprint.into();
        self
    }

    pub fn with_model_modalities(mut self, modalities: BTreeSet<ModelModality>) -> Self {
        self.model_modalities = modalities;
        self
    }

    pub fn verify_provider_resume_fingerprint(&self, current: &str) -> Result<()> {
        ensure!(
            !self.provider_resume_fingerprint.is_empty(),
            "run `{}` has no provider resume fingerprint",
            self.id
        );
        ensure!(
            self.provider_resume_fingerprint == current,
            "run `{}` provider configuration differs from its recorded resume fingerprint",
            self.id
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPaths {
    pub directory: PathBuf,
    pub execution_lock: PathBuf,
    pub metadata: PathBuf,
    pub messages: PathBuf,
    pub pending_inputs: PathBuf,
    pub events: PathBuf,
    pub final_output: PathBuf,
    pub artifacts: PathBuf,
}

#[derive(Clone)]
pub struct RunDirStore {
    workspace: PathBuf,
    write_lock: Arc<Mutex<HashMap<String, MessageCursor>>>,
    input_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MessageCursor {
    next_seq: u64,
}

impl RunDirStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            write_lock: Arc::new(Mutex::new(HashMap::new())),
            input_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn paths(&self, run_id: &str) -> RunPaths {
        let directory = self
            .workspace
            .join(".fiasco")
            .join("runs")
            .join(safe_component(run_id));
        RunPaths {
            execution_lock: directory.join(".run.lock"),
            metadata: directory.join("run.json"),
            messages: directory.join("messages.jsonl"),
            pending_inputs: directory.join("pending_inputs.jsonl"),
            events: directory.join("events.jsonl"),
            final_output: directory.join("final.md"),
            artifacts: directory.join("artifacts"),
            directory,
        }
    }

    pub async fn create_run(&self, run: &RunRecord) -> Result<RunPaths> {
        let mut sequences = self.write_lock.lock().await;
        let paths = self.paths(&run.id);
        if run.message_format != MESSAGE_FORMAT {
            bail!(
                "unsupported message format {}; expected {MESSAGE_FORMAT}",
                run.message_format
            );
        }
        ensure!(
            run.version == RUN_RECORD_VERSION,
            "unsupported run record version {}; expected {RUN_RECORD_VERSION}",
            run.version
        );
        ensure!(
            run.model_modalities.contains(&ModelModality::Text),
            "run record model modalities must include text"
        );
        validate_run_parentage(run)?;
        if paths.metadata.exists() {
            bail!("run `{}` already exists", run.id);
        }
        tokio::fs::create_dir_all(&paths.artifacts)
            .await
            .with_context(|| format!("create run directory {}", paths.directory.display()))?;
        sync_directory_chain(&paths.directory, &self.workspace).await?;
        message_log::initialize(&paths.directory, &paths.messages).await?;
        write_json_atomic(&paths.metadata, run).await?;
        sequences.insert(run.id.clone(), MessageCursor { next_seq: 1 });
        Ok(paths)
    }

    pub async fn update_state(&self, run_id: &str, state: RunState) -> Result<RunRecord> {
        let _guard = self.write_lock.lock().await;
        let path = self.paths(run_id).metadata;
        let mut run: RunRecord = read_json(&path).await?;
        run.state = state;
        run.updated_at = Utc::now();
        write_json_atomic(&path, &run).await?;
        Ok(run)
    }

    pub async fn verify_tool_schema(&self, run_id: &str, tool_schema_sha256: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let path = self.paths(run_id).metadata;
        let mut run: RunRecord = read_json(&path).await?;
        if run.tool_schema_sha256.is_empty() {
            run.tool_schema_sha256 = tool_schema_sha256.to_owned();
            run.updated_at = Utc::now();
            return write_json_atomic(&path, &run).await;
        }
        ensure!(
            run.tool_schema_sha256 == tool_schema_sha256,
            "run `{run_id}` tool schemas differ from its recorded capability profile"
        );
        Ok(())
    }

    pub async fn write_final(&self, run_id: &str, output: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        tokio::fs::write(&paths.final_output, output)
            .await
            .with_context(|| format!("write final output {}", paths.final_output.display()))
    }

    pub async fn load_run(&self, run_id: &str) -> Result<RunRecord> {
        let run: RunRecord = read_json(&self.paths(run_id).metadata).await?;
        ensure!(
            run.version == RUN_RECORD_VERSION,
            "unsupported run record version {}; expected {RUN_RECORD_VERSION}",
            run.version
        );
        ensure!(
            run.message_format == MESSAGE_FORMAT,
            "unsupported message format {}; expected {MESSAGE_FORMAT}",
            run.message_format
        );
        ensure!(
            run.model_modalities.contains(&ModelModality::Text),
            "run record model modalities must include text"
        );
        validate_run_parentage(&run)?;
        Ok(run)
    }

    /// Hold this lease for the full lifetime of a running or resumed loop.
    /// It prevents two processes from advancing the same transcript at once.
    pub async fn acquire_run_lease(&self, run_id: &str) -> Result<RunLease> {
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let run_id = run_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(&paths.execution_lock)
                .with_context(|| {
                    format!("open run execution lock {}", paths.execution_lock.display())
                })?;
            if let Err(source) = file.try_lock() {
                return Err(RunLeaseBusy {
                    run_id,
                    source: source.into(),
                }
                .into());
            }
            Ok(RunLease {
                _file: Arc::new(file),
            })
        })
        .await
        .context("join run execution lock task")?
    }

    pub fn event_sink(&self) -> SharedEventSink {
        Arc::new(self.clone())
    }
}

fn validate_run_parentage(run: &RunRecord) -> Result<()> {
    match (run.profile.as_str(), run.parent_run_id.as_ref()) {
        ("root", None) => Ok(()),
        ("general_task_delegating" | "general_task_leaf", Some(_)) => Ok(()),
        ("root", Some(_)) => bail!("root run cannot have a parent"),
        ("general_task_delegating" | "general_task_leaf", None) => {
            bail!("GeneralTask run must have a parent")
        }
        (profile, _) => bail!("unknown run profile `{profile}`"),
    }
}

#[derive(Debug, Clone)]
pub struct RunLease {
    _file: Arc<std::fs::File>,
}

#[derive(Debug)]
pub(crate) struct RunLeaseBusy {
    run_id: String,
    source: std::io::Error,
}

impl fmt::Display for RunLeaseBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "run `{}` is already being executed by another process",
            self.run_id
        )
    }
}

impl std::error::Error for RunLeaseBusy {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[async_trait]
impl EventSink for RunDirStore {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        if matches!(
            &event.kind,
            RuntimeEventKind::ModelDelta { .. } | RuntimeEventKind::ModelReasoningDelta { .. }
        ) {
            return Ok(());
        }
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(&event.run_id);
        ensure_run_exists(&paths).await?;
        append_json_line(&paths.events, event).await
    }
}

pub(super) async fn ensure_run_exists(paths: &RunPaths) -> Result<()> {
    if !tokio::fs::try_exists(&paths.metadata).await? {
        bail!("run does not exist: {}", paths.directory.display());
    }
    Ok(())
}

pub(super) async fn append_json_line(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(value).context("serialize JSONL record")?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {} for append", path.display()))?;
    file.write_all(&line)
        .await
        .with_context(|| format!("append {}", path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("flush {}", path.display()))
}

async fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

async fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).context("serialize JSON")?;
    let temporary = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .await
        .with_context(|| format!("open {} for atomic write", temporary.display()))?;
    file.write_all(&bytes)
        .await
        .with_context(|| format!("write {}", temporary.display()))?;
    file.flush().await?;
    file.sync_all()
        .await
        .with_context(|| format!("sync {}", temporary.display()))?;
    drop(file);
    tokio::fs::rename(&temporary, path)
        .await
        .with_context(|| format!("replace {}", path.display()))?;
    sync_parent_directory(path).await
}

#[cfg(unix)]
async fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?
        .to_owned();
    tokio::task::spawn_blocking(move || {
        let directory = std::fs::File::open(&parent)
            .with_context(|| format!("open {} for directory sync", parent.display()))?;
        directory
            .sync_all()
            .with_context(|| format!("sync directory {}", parent.display()))
    })
    .await
    .context("join directory sync task")?
}

#[cfg(not(unix))]
async fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
async fn sync_directory_chain(path: &Path, through: &Path) -> Result<()> {
    let mut current = tokio::fs::canonicalize(path)
        .await
        .with_context(|| format!("resolve directory {}", path.display()))?;
    let through = tokio::fs::canonicalize(through)
        .await
        .with_context(|| format!("resolve workspace {}", through.display()))?;
    tokio::task::spawn_blocking(move || {
        loop {
            let directory = std::fs::File::open(&current)
                .with_context(|| format!("open {} for directory sync", current.display()))?;
            directory
                .sync_all()
                .with_context(|| format!("sync directory {}", current.display()))?;
            if current == through {
                return Ok(());
            }
            ensure!(
                current.starts_with(&through),
                "run directory {} is outside workspace {}",
                current.display(),
                through.display()
            );
            current = current
                .parent()
                .with_context(|| format!("{} has no parent directory", current.display()))?
                .to_owned();
        }
    })
    .await
    .context("join directory hierarchy sync task")?
}

#[cfg(not(unix))]
async fn sync_directory_chain(_path: &Path, _through: &Path) -> Result<()> {
    Ok(())
}

fn safe_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}
