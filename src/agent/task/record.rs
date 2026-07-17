use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::artifact::ResultMetadata;

const TASK_RECORD_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    /// The process stopped while a non-resumable operation was in flight. Its
    /// side effects are unknown, so recovery must never execute it again.
    Interrupted,
}

impl BackgroundTaskState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

/// Durable coordination state between a parent run and one background unit of
/// work. Agent transcripts live in the child run; this record deliberately
/// does not copy them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundTaskRecord {
    pub version: u32,
    pub id: String,
    /// `agent` or `tool`.
    pub kind: String,
    /// Agent profile or tool name.
    pub name: String,
    pub state: BackgroundTaskState,
    pub result: Option<BackgroundTaskOutput>,
    pub error: Option<String>,
    pub child_run_id: Option<String>,
    /// Needed only when an agent task was durably queued but its child run was
    /// not created before the process stopped.
    pub prompt: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundTaskOutput {
    pub content: String,
    pub metadata: ResultMetadata,
}

impl BackgroundTaskRecord {
    pub(super) fn queued_tool(id: String, name: String) -> Self {
        Self {
            version: TASK_RECORD_VERSION,
            id,
            kind: "tool".to_owned(),
            name,
            state: BackgroundTaskState::Queued,
            result: None,
            error: None,
            child_run_id: None,
            prompt: None,
            created_at: Utc::now(),
        }
    }

    pub(super) fn queued_agent(
        id: String,
        profile: String,
        child_run_id: String,
        prompt: String,
    ) -> Self {
        Self {
            version: TASK_RECORD_VERSION,
            id,
            kind: "agent".to_owned(),
            name: profile,
            state: BackgroundTaskState::Queued,
            result: None,
            error: None,
            child_run_id: Some(child_run_id),
            prompt: Some(prompt),
            created_at: Utc::now(),
        }
    }

    pub fn model_content(&self) -> String {
        match self.state {
            BackgroundTaskState::Completed => self
                .result
                .as_ref()
                .map(|result| result.content.clone())
                .unwrap_or_default(),
            BackgroundTaskState::Failed => self
                .result
                .as_ref()
                .map(|result| result.content.clone())
                .unwrap_or_else(|| {
                    format!(
                        "background task failed: {}",
                        self.error.as_deref().unwrap_or("unknown error")
                    )
                }),
            BackgroundTaskState::Cancelled => "background task was cancelled".to_owned(),
            BackgroundTaskState::Interrupted => format!(
                "background task was interrupted; its side effects are unknown: {}",
                self.error.as_deref().unwrap_or("process stopped")
            ),
            BackgroundTaskState::Queued | BackgroundTaskState::Running => {
                "background task is still running".to_owned()
            }
        }
    }

    pub fn result_metadata(&self) -> ResultMetadata {
        self.result
            .as_ref()
            .map(|result| result.metadata.clone())
            .unwrap_or_else(ResultMetadata::empty)
    }

    pub fn status(&self) -> &'static str {
        match self.state {
            BackgroundTaskState::Queued => "queued",
            BackgroundTaskState::Running => "running",
            BackgroundTaskState::Completed => "completed",
            BackgroundTaskState::Failed => "failed",
            BackgroundTaskState::Cancelled => "cancelled",
            BackgroundTaskState::Interrupted => "interrupted",
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.version == TASK_RECORD_VERSION,
            "unsupported task record version {}",
            self.version
        );
        ensure!(!self.id.is_empty(), "task id must not be empty");
        match self.kind.as_str() {
            "tool" => ensure!(
                self.child_run_id.is_none() && self.prompt.is_none(),
                "tool task {} cannot reference a child run or prompt",
                self.id
            ),
            "agent" => ensure!(
                self.child_run_id.is_some() && self.prompt.is_some(),
                "agent task {} must reference a child run and prompt",
                self.id
            ),
            kind => bail!("unknown task kind `{kind}` in task {}", self.id),
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct TaskRecordStore {
    directory: PathBuf,
}

impl TaskRecordStore {
    pub(super) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(super) async fn load(&self) -> Result<BTreeMap<String, BackgroundTaskRecord>> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read task directory {}", self.directory.display()));
            }
        };
        let mut records = BTreeMap::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = tokio::fs::read(&path)
                .await
                .with_context(|| format!("read task record {}", path.display()))?;
            let record: BackgroundTaskRecord = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse task record {}", path.display()))?;
            record.validate()?;
            let file_id = path.file_stem().and_then(|value| value.to_str());
            ensure!(
                file_id == Some(record.id.as_str()),
                "task record id `{}` does not match file {}",
                record.id,
                path.display()
            );
            ensure!(
                records.insert(record.id.clone(), record).is_none(),
                "duplicate task record"
            );
        }
        Ok(records)
    }

    pub(super) async fn write(&self, record: &BackgroundTaskRecord) -> Result<()> {
        record.validate()?;
        tokio::fs::create_dir_all(&self.directory)
            .await
            .with_context(|| format!("create task directory {}", self.directory.display()))?;
        if let Some(parent) = self.directory.parent() {
            sync_directory(parent).await?;
        }
        let path = self.directory.join(format!("{}.json", record.id));
        let temporary = self.directory.join(format!("{}.json.tmp", record.id));
        let bytes = serde_json::to_vec_pretty(record).context("serialize task record")?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .await
            .with_context(|| format!("open task record {}", temporary.display()))?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, &path)
            .await
            .with_context(|| format!("replace task record {}", path.display()))?;
        sync_directory(&self.directory).await
    }
}

#[cfg(unix)]
async fn sync_directory(path: &std::path::Path) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path)
            .with_context(|| format!("open task directory {} for sync", path.display()))?
            .sync_all()
            .with_context(|| format!("sync task directory {}", path.display()))
    })
    .await
    .context("join task directory sync")?
}

#[cfg(not(unix))]
async fn sync_directory(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_records_round_trip_and_ignore_temporary_files() {
        let workspace = tempfile::tempdir().unwrap();
        let store = TaskRecordStore::new(workspace.path().join("tasks"));
        let mut record = BackgroundTaskRecord::queued_agent(
            "task-1".to_owned(),
            "general-task".to_owned(),
            "child-1".to_owned(),
            "inspect the workspace".to_owned(),
        );
        record.state = BackgroundTaskState::Completed;
        record.result = Some(BackgroundTaskOutput {
            content: "child result".to_owned(),
            metadata: ResultMetadata {
                artifact: None,
                preview_bytes: 12,
            },
        });
        store.write(&record).await.unwrap();
        tokio::fs::write(workspace.path().join("tasks/orphan.json.tmp"), b"{")
            .await
            .unwrap();

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        let loaded = &loaded["task-1"];
        assert_eq!(loaded.child_run_id.as_deref(), Some("child-1"));
        assert_eq!(loaded.prompt.as_deref(), Some("inspect the workspace"));
        assert_eq!(loaded.result_metadata().preview_bytes, 12);
    }

    #[tokio::test]
    async fn task_record_filename_must_match_its_id() {
        let workspace = tempfile::tempdir().unwrap();
        let store = TaskRecordStore::new(workspace.path().join("tasks"));
        let record = BackgroundTaskRecord::queued_tool("task-1".to_owned(), "read".to_owned());
        tokio::fs::create_dir_all(workspace.path().join("tasks"))
            .await
            .unwrap();
        tokio::fs::write(
            workspace.path().join("tasks/wrong.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .await
        .unwrap();

        assert!(
            store
                .load()
                .await
                .unwrap_err()
                .to_string()
                .contains("does not match")
        );
    }
}
