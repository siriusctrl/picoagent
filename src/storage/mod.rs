use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::Mutex};

use crate::{
    events::{EventSink, RuntimeEvent, SharedEventSink},
    model::Message,
};

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
    pub events: PathBuf,
    pub final_output: PathBuf,
    pub artifacts: PathBuf,
}

#[derive(Clone)]
pub struct RunDirStore {
    workspace: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl RunDirStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            write_lock: Arc::new(Mutex::new(())),
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
            events: directory.join("events.jsonl"),
            final_output: directory.join("final.md"),
            artifacts: directory.join("artifacts"),
            directory,
        }
    }

    pub async fn create_run(&self, run: &RunRecord) -> Result<RunPaths> {
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(&run.id);
        if paths.metadata.exists() {
            bail!("run `{}` already exists", run.id);
        }
        tokio::fs::create_dir_all(&paths.artifacts)
            .await
            .with_context(|| format!("create run directory {}", paths.directory.display()))?;
        write_json_atomic(&paths.metadata, run).await?;
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

    pub async fn append_message(&self, run_id: &str, message: &Message) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        append_json_line(&paths.messages, message).await
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

    pub async fn load_messages(&self, run_id: &str) -> Result<Vec<Message>> {
        let path = self.paths(run_id).messages;
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("parse stored message"))
            .collect()
    }

    pub fn event_sink(&self) -> SharedEventSink {
        Arc::new(self.clone())
    }
}

#[async_trait]
impl EventSink for RunDirStore {
    async fn emit(&self, event: &RuntimeEvent) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(&event.run_id);
        ensure_run_exists(&paths).await?;
        append_json_line(&paths.events, event).await
    }
}

async fn ensure_run_exists(paths: &RunPaths) -> Result<()> {
    if !tokio::fs::try_exists(&paths.metadata).await? {
        bail!("run does not exist: {}", paths.directory.display());
    }
    Ok(())
}

async fn append_json_line(path: &Path, value: &impl Serialize) -> Result<()> {
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
    tokio::fs::write(&temporary, bytes)
        .await
        .with_context(|| format!("write {}", temporary.display()))?;
    tokio::fs::rename(&temporary, path)
        .await
        .with_context(|| format!("replace {}", path.display()))
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
