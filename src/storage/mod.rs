use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::Mutex};

use crate::events::{EventSink, RuntimeEvent, RuntimeEventKind, SharedEventSink};

mod message_log;
mod trajectory;

pub use trajectory::CompactionCheckpoint;

pub const MESSAGE_FORMAT: &str = "openai-chat-compatible";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub version: u32,
    pub id: String,
    pub parent_run_id: Option<String>,
    pub state: RunState,
    pub prompt: String,
    pub provider: String,
    pub model: String,
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
            version: 1,
            id: id.into(),
            parent_run_id,
            state: RunState::Queued,
            prompt: prompt.into(),
            provider: provider.into(),
            model: model.into(),
            message_format: MESSAGE_FORMAT.to_owned(),
            cwd,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPaths {
    pub directory: PathBuf,
    pub metadata: PathBuf,
    pub messages: PathBuf,
    pub message_metadata: PathBuf,
    pub compactions: PathBuf,
    pub events: PathBuf,
    pub final_output: PathBuf,
    pub artifacts: PathBuf,
}

#[derive(Clone)]
pub struct RunDirStore {
    workspace: PathBuf,
    write_lock: Arc<Mutex<HashMap<String, MessageCursor>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MessageCursor {
    next_seq: u64,
    messages_len: u64,
    metadata_len: u64,
}

impl RunDirStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            write_lock: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn paths(&self, run_id: &str) -> RunPaths {
        let directory = self
            .workspace
            .join(".pico")
            .join("runs")
            .join(safe_component(run_id));
        RunPaths {
            metadata: directory.join("run.json"),
            messages: directory.join("messages.jsonl"),
            message_metadata: directory.join("message_metadata.jsonl"),
            compactions: directory.join("compactions.jsonl"),
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
        if paths.metadata.exists() {
            bail!("run `{}` already exists", run.id);
        }
        tokio::fs::create_dir_all(&paths.artifacts)
            .await
            .with_context(|| format!("create run directory {}", paths.directory.display()))?;
        sync_directory_chain(&paths.directory, &self.workspace).await?;
        message_log::initialize(&paths.directory, &paths.messages, &paths.message_metadata).await?;
        write_json_atomic(&paths.metadata, run).await?;
        sequences.insert(
            run.id.clone(),
            MessageCursor {
                next_seq: 1,
                messages_len: 0,
                metadata_len: 0,
            },
        );
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

    pub async fn write_final(&self, run_id: &str, output: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        tokio::fs::write(&paths.final_output, output)
            .await
            .with_context(|| format!("write final output {}", paths.final_output.display()))
    }

    pub async fn load_run(&self, run_id: &str) -> Result<RunRecord> {
        read_json(&self.paths(run_id).metadata).await
    }

    pub fn event_sink(&self) -> SharedEventSink {
        Arc::new(self.clone())
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
