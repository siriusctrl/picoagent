use anyhow::{Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::artifact::ResultMetadata;

mod store;
pub(super) use store::TaskRecordStore;

const TASK_RECORD_VERSION: u32 = 12;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskState {
    Queued,
    Running,
    /// A reusable agent task is waiting for more input.
    Idle,
    Completed,
    Failed,
    Cancelled,
    /// The process stopped while a non-resumable operation was in flight. Its
    /// side effects are unknown, so recovery must never execute it again.
    Interrupted,
    /// A reusable agent task was explicitly closed.
    Closed,
}

impl BackgroundTaskState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted | Self::Closed
        )
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskOutputStatus {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl BackgroundTaskOutputStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
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
    /// Original provider tool-call id for the call which created this task.
    /// It stays internal and lets recovery reconstruct the missing status-less
    /// acknowledgement without replaying the call.
    pub origin_call_id: String,
    pub state: BackgroundTaskState,
    /// Ordered immutable outputs. Ordinary tool tasks have at most one;
    /// reusable agent tasks may produce one after every activation.
    pub outputs: Vec<BackgroundTaskOutput>,
    /// Follow-up messages wait for the current agent activity to settle before
    /// they are moved into the child run's ordinary pending-input queue.
    pub pending_followups: Vec<PendingTaskInput>,
    /// `task_stop` pauses automatic activation without ending the reusable
    /// agent lifetime. The next explicit `task_send` clears this flag.
    pub paused: bool,
    pub child_run_id: Option<String>,
    /// Capability fixed before an agent child starts. Recovery must not derive
    /// it again from the current runtime depth configuration.
    pub child_remaining_delegation_depth: Option<usize>,
    /// Complete isolated assignment retained for child validation and task
    /// lifecycle events.
    pub prompt: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundTaskOutput {
    pub seq: u64,
    pub status: BackgroundTaskOutputStatus,
    pub content: String,
    pub metadata: ResultMetadata,
}

impl BackgroundTaskOutput {
    pub fn model_content(&self) -> String {
        self.content.clone()
    }

    pub fn result_metadata(&self) -> ResultMetadata {
        self.metadata.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingTaskInput {
    pub id: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl BackgroundTaskRecord {
    pub(crate) fn queued_tool(id: String, name: String, origin_call_id: String) -> Self {
        Self {
            version: TASK_RECORD_VERSION,
            id,
            kind: "tool".to_owned(),
            name,
            origin_call_id,
            state: BackgroundTaskState::Queued,
            outputs: Vec::new(),
            pending_followups: Vec::new(),
            paused: false,
            child_run_id: None,
            child_remaining_delegation_depth: None,
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
        let origin_call_id = format!("delegate-{id}");
        Self::queued_agent_with_origin(
            id,
            name,
            child_run_id,
            prompt,
            child_remaining_delegation_depth,
            origin_call_id,
        )
    }

    pub(super) fn queued_agent_with_origin(
        id: String,
        name: String,
        child_run_id: String,
        prompt: String,
        child_remaining_delegation_depth: usize,
        origin_call_id: String,
    ) -> Self {
        Self {
            version: TASK_RECORD_VERSION,
            id,
            kind: "agent".to_owned(),
            name,
            origin_call_id,
            state: BackgroundTaskState::Queued,
            outputs: Vec::new(),
            pending_followups: Vec::new(),
            paused: false,
            child_run_id: Some(child_run_id),
            child_remaining_delegation_depth: Some(child_remaining_delegation_depth),
            prompt: Some(prompt),
            created_at: Utc::now(),
        }
    }

    pub fn status(&self) -> &'static str {
        match self.state {
            BackgroundTaskState::Queued => "queued",
            BackgroundTaskState::Running => "running",
            BackgroundTaskState::Idle => "idle",
            BackgroundTaskState::Completed => "completed",
            BackgroundTaskState::Failed => "failed",
            BackgroundTaskState::Cancelled => "cancelled",
            BackgroundTaskState::Interrupted => "interrupted",
            BackgroundTaskState::Closed => "closed",
        }
    }

    pub fn next_output_seq(&self) -> u64 {
        self.outputs
            .last()
            .map_or(1, |output| output.seq.saturating_add(1))
    }

    pub fn model_content(&self) -> String {
        self.outputs
            .last()
            .map(BackgroundTaskOutput::model_content)
            .unwrap_or_else(|| "background task has no output".to_owned())
    }

    pub fn result_metadata(&self) -> ResultMetadata {
        self.outputs
            .last()
            .map(BackgroundTaskOutput::result_metadata)
            .unwrap_or_else(ResultMetadata::empty)
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
        ensure!(
            !self.origin_call_id.is_empty(),
            "task {} must reference its original provider call id",
            self.id
        );
        for (index, output) in self.outputs.iter().enumerate() {
            ensure!(
                output.seq == index as u64 + 1,
                "task {} output sequence is not contiguous",
                self.id
            );
        }
        match self.kind.as_str() {
            "tool" => {
                ensure!(
                    self.child_run_id.is_none()
                        && self.child_remaining_delegation_depth.is_none()
                        && self.prompt.is_none()
                        && self.pending_followups.is_empty()
                        && !self.paused,
                    "tool task {} cannot reference child state",
                    self.id
                );
                ensure!(
                    self.outputs.len() <= 1,
                    "tool task {} cannot produce multiple outputs",
                    self.id
                );
            }
            "agent" => {
                ensure!(
                    self.child_run_id.is_some()
                        && self.child_remaining_delegation_depth.is_some()
                        && self.prompt.is_some(),
                    "agent task {} must reference a child run, capability, and prompt",
                    self.id
                );
                ensure!(
                    !self.paused || self.state == BackgroundTaskState::Idle,
                    "paused agent task {} must be idle",
                    self.id
                );
            }
            kind => bail!("unknown task kind `{kind}` in task {}", self.id),
        }
        Ok(())
    }
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
        record.state = BackgroundTaskState::Idle;
        record.outputs.push(BackgroundTaskOutput {
            seq: 1,
            status: BackgroundTaskOutputStatus::Completed,
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
        assert_eq!(loaded.origin_call_id, "delegate-task-1");
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

        assert_eq!(loaded["task-1"].origin_call_id, "provider-call-7");
    }

    #[test]
    fn every_task_kind_requires_an_original_call_id() {
        let mut tool = BackgroundTaskRecord::queued_tool(
            "task-1".to_owned(),
            "read".to_owned(),
            "provider-call-7".to_owned(),
        );
        tool.origin_call_id.clear();
        assert!(
            tool.validate()
                .unwrap_err()
                .to_string()
                .contains("original provider call id")
        );

        let mut agent = BackgroundTaskRecord::queued_agent(
            "task-2".to_owned(),
            "general-task".to_owned(),
            "child-1".to_owned(),
            "inspect".to_owned(),
            0,
        );
        agent.origin_call_id.clear();
        assert!(
            agent
                .validate()
                .unwrap_err()
                .to_string()
                .contains("original provider call id")
        );
    }
}
