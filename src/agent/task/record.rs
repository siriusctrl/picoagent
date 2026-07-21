use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{artifact::ResultMetadata, storage::DelegateContext};

const TASK_RECORD_VERSION: u32 = 8;

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
    /// Model-supplied agent task label or promoted tool name.
    pub name: String,
    /// Original provider tool-call id for a promoted ordinary tool. This is
    /// absent for delegated agent tasks.
    pub origin_call_id: Option<String>,
    pub state: BackgroundTaskState,
    pub result: Option<BackgroundTaskOutput>,
    pub error: Option<String>,
    pub child_run_id: Option<String>,
    /// Capability fixed before an agent child starts. Recovery must not derive
    /// it again from the current runtime depth configuration.
    pub child_remaining_delegation_depth: Option<usize>,
    /// Context inheritance selected by the delegate call. Promoted ordinary
    /// tools do not have a delegate context.
    pub delegate_context: Option<DelegateContext>,
    /// Frozen parent trajectory boundary for a forked child.
    pub fork_parent_message_seq: Option<u64>,
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
    pub(crate) fn queued_tool(id: String, name: String, origin_call_id: String) -> Self {
        Self {
            version: TASK_RECORD_VERSION,
            id,
            kind: "tool".to_owned(),
            name,
            origin_call_id: Some(origin_call_id),
            state: BackgroundTaskState::Queued,
            result: None,
            error: None,
            child_run_id: None,
            child_remaining_delegation_depth: None,
            delegate_context: None,
            fork_parent_message_seq: None,
            prompt: None,
            created_at: Utc::now(),
        }
    }

    #[cfg(test)]
    pub(super) fn queued_agent(
        id: String,
        name: String,
        child_run_id: String,
        prompt: String,
        child_remaining_delegation_depth: usize,
    ) -> Self {
        Self::queued_agent_with_context(
            id,
            name,
            child_run_id,
            prompt,
            child_remaining_delegation_depth,
            DelegateContext::Fresh,
            None,
        )
    }

    pub(super) fn queued_agent_with_context(
        id: String,
        name: String,
        child_run_id: String,
        prompt: String,
        child_remaining_delegation_depth: usize,
        delegate_context: DelegateContext,
        fork_parent_message_seq: Option<u64>,
    ) -> Self {
        Self {
            version: TASK_RECORD_VERSION,
            id,
            kind: "agent".to_owned(),
            name,
            origin_call_id: None,
            state: BackgroundTaskState::Queued,
            result: None,
            error: None,
            child_run_id: Some(child_run_id),
            child_remaining_delegation_depth: Some(child_remaining_delegation_depth),
            delegate_context: Some(delegate_context),
            fork_parent_message_seq,
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
            BackgroundTaskState::Cancelled => format!(
                "background task was cancelled: {}",
                self.error.as_deref().unwrap_or("no reason recorded")
            ),
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
        ensure!(!self.name.trim().is_empty(), "task name must not be empty");
        ensure!(
            !self.name.chars().any(char::is_control),
            "task name must not contain control characters"
        );
        match self.kind.as_str() {
            "tool" => {
                ensure!(
                    self.origin_call_id
                        .as_deref()
                        .is_some_and(|call_id| !call_id.is_empty()),
                    "tool task {} must reference its original tool-call id",
                    self.id
                );
                ensure!(
                    self.child_run_id.is_none()
                        && self.child_remaining_delegation_depth.is_none()
                        && self.delegate_context.is_none()
                        && self.fork_parent_message_seq.is_none()
                        && self.prompt.is_none(),
                    "tool task {} cannot reference child state",
                    self.id
                );
            }
            "agent" => {
                ensure!(
                    self.origin_call_id.is_none(),
                    "agent task {} cannot reference a tool-call id",
                    self.id
                );
                ensure!(
                    self.child_run_id.is_some()
                        && self.child_remaining_delegation_depth.is_some()
                        && self.delegate_context.is_some()
                        && self.prompt.is_some(),
                    "agent task {} must reference a child run, capability, and prompt",
                    self.id
                );
                match (self.delegate_context, self.fork_parent_message_seq) {
                    (Some(DelegateContext::Fresh), None) => {}
                    (Some(DelegateContext::Fork), Some(seq)) if seq > 0 => {}
                    (Some(DelegateContext::Fresh), Some(_)) => {
                        bail!("fresh agent task {} cannot have a fork boundary", self.id)
                    }
                    (Some(DelegateContext::Fork), _) => bail!(
                        "forked agent task {} must have a positive parent message boundary",
                        self.id
                    ),
                    (None, _) => bail!("agent task {} has no delegate context", self.id),
                }
            }
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
            0,
        );
        record.state = BackgroundTaskState::Completed;
        record.result = Some(BackgroundTaskOutput {
            content: "child result".to_owned(),
            metadata: ResultMetadata::empty(),
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
        assert_eq!(loaded.result_metadata(), ResultMetadata::empty());
    }

    #[tokio::test]
    async fn task_record_filename_must_match_its_id() {
        let workspace = tempfile::tempdir().unwrap();
        let store = TaskRecordStore::new(workspace.path().join("tasks"));
        let record = BackgroundTaskRecord::queued_tool(
            "task-1".to_owned(),
            "read".to_owned(),
            "call-1".to_owned(),
        );
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

    #[tokio::test]
    async fn promoted_tool_record_round_trips_its_original_call_id() {
        let workspace = tempfile::tempdir().unwrap();
        let store = TaskRecordStore::new(workspace.path().join("tasks"));
        let record = BackgroundTaskRecord::queued_tool(
            "task-1".to_owned(),
            "read".to_owned(),
            "provider-call-7".to_owned(),
        );

        store.write(&record).await.unwrap();
        let loaded = store.load().await.unwrap();

        assert_eq!(
            loaded["task-1"].origin_call_id.as_deref(),
            Some("provider-call-7")
        );
    }

    #[test]
    fn task_kind_validates_original_call_id_ownership() {
        let mut tool = BackgroundTaskRecord::queued_tool(
            "task-1".to_owned(),
            "read".to_owned(),
            "provider-call-7".to_owned(),
        );
        tool.origin_call_id = None;
        assert!(
            tool.validate()
                .unwrap_err()
                .to_string()
                .contains("original tool-call id")
        );

        let mut agent = BackgroundTaskRecord::queued_agent(
            "task-2".to_owned(),
            "general-task".to_owned(),
            "child-1".to_owned(),
            "inspect".to_owned(),
            0,
        );
        agent.origin_call_id = Some("provider-call-8".to_owned());
        assert!(
            agent
                .validate()
                .unwrap_err()
                .to_string()
                .contains("cannot reference a tool-call id")
        );
    }

    #[test]
    fn agent_task_requires_a_context_consistent_with_its_fork_boundary() {
        let fork = BackgroundTaskRecord::queued_agent_with_context(
            "task-1".to_owned(),
            "fork".to_owned(),
            "child-1".to_owned(),
            "inspect".to_owned(),
            0,
            DelegateContext::Fork,
            Some(4),
        );
        fork.validate().unwrap();

        let mut missing_boundary = fork.clone();
        missing_boundary.fork_parent_message_seq = None;
        assert!(
            missing_boundary
                .validate()
                .unwrap_err()
                .to_string()
                .contains("positive parent message boundary")
        );

        let mut fresh_with_boundary = fork;
        fresh_with_boundary.delegate_context = Some(DelegateContext::Fresh);
        assert!(
            fresh_with_boundary
                .validate()
                .unwrap_err()
                .to_string()
                .contains("fresh agent task")
        );
    }
}
