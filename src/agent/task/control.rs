use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    model::openai_chat::project_chat_message,
    storage::RunState,
};

use super::{BackgroundTaskState, PendingTaskInput, TaskManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSendMode {
    Steer,
    Followup,
}

impl TaskSendMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Followup => "followup",
        }
    }
}

impl TaskManager {
    pub async fn inspect(
        &self,
        task_id: &str,
        before_seq: Option<u64>,
        limit: usize,
    ) -> Result<Value> {
        let record = self.get(task_id).await?;
        ensure!(record.kind == "agent", "task `{task_id}` is not an agent");
        let child_run_id = record
            .child_run_id
            .as_deref()
            .context("agent task is missing child_run_id")?;
        let path = self.store.paths(child_run_id).metadata;
        let trajectory = if tokio::fs::try_exists(&path).await? {
            self.store.load_trajectory(child_run_id).await?
        } else {
            Vec::new()
        };
        let before_seq = before_seq.unwrap_or(u64::MAX);
        let eligible = trajectory
            .iter()
            .filter(|message| message.seq < before_seq)
            .collect::<Vec<_>>();
        let start = eligible.len().saturating_sub(limit);
        let messages = eligible[start..]
            .iter()
            .map(|record| {
                json!({
                    "seq": record.seq,
                    "message": project_chat_message(&record.message),
                })
            })
            .collect::<Vec<_>>();
        let has_earlier = start > 0;
        let next_before_seq = has_earlier
            .then(|| eligible[start].seq)
            .map_or(Value::Null, Value::from);
        Ok(json!({
            "task_id": record.id,
            "name": record.name,
            "status": record.status(),
            "messages": messages,
            "has_earlier": has_earlier,
            "next_before_seq": next_before_seq,
        }))
    }

    pub async fn send(
        self: &std::sync::Arc<Self>,
        task_id: &str,
        message: String,
        mode: TaskSendMode,
    ) -> Result<Value> {
        ensure!(!message.trim().is_empty(), "task message must not be empty");
        let mut records = self.records.lock().await;
        let record = records
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))?;
        ensure!(record.kind == "agent", "task `{task_id}` is not an agent");
        ensure!(
            !record.state.is_terminal() && record.state != BackgroundTaskState::Closed,
            "agent task `{task_id}` is already {}",
            record.status()
        );
        let child_run_id = record
            .child_run_id
            .as_deref()
            .context("agent task is missing child_run_id")?;
        let child_path = self.store.paths(child_run_id).metadata;
        if tokio::fs::try_exists(&child_path).await? {
            let child = self.store.load_run(child_run_id).await?;
            ensure!(
                matches!(
                    child.state,
                    RunState::Queued | RunState::Running | RunState::Idle
                ),
                "child run `{child_run_id}` is already {:?}",
                child.state
            );
        }
        let input_id = format!("input_{}", ulid::Ulid::new());
        let accepted_as = match (record.state, mode) {
            (BackgroundTaskState::Queued | BackgroundTaskState::Running, TaskSendMode::Steer) => {
                self.store
                    .enqueue_user_input_with_id(child_run_id, input_id.clone(), message)
                    .await?;
                "steered"
            }
            (
                BackgroundTaskState::Queued | BackgroundTaskState::Running,
                TaskSendMode::Followup,
            ) => {
                let mut updated = record.clone();
                updated.pending_followups.push(PendingTaskInput {
                    id: input_id.clone(),
                    message,
                    created_at: chrono::Utc::now(),
                });
                self.persist(&updated).await?;
                records.insert(task_id.to_owned(), updated);
                "queued_followup"
            }
            (BackgroundTaskState::Idle, _) => {
                let mut updated = record.clone();
                updated.paused = false;
                updated.pending_followups.push(PendingTaskInput {
                    id: input_id.clone(),
                    message,
                    created_at: chrono::Utc::now(),
                });
                self.persist(&updated).await?;
                records.insert(task_id.to_owned(), updated);
                "started"
            }
            _ => anyhow::bail!("agent task `{task_id}` is already {}", record.status()),
        };
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::SubagentMessageQueued {
                    task_id: task_id.to_owned(),
                    child_run_id: child_run_id.to_owned(),
                    input_id: input_id.clone(),
                    mode: mode.as_str().to_owned(),
                },
            ))
            .await;
        drop(records);
        if record.state == BackgroundTaskState::Idle {
            let activated = self.activate_agent_if_pending(task_id).await?;
            if !activated {
                let current = self.get(task_id).await?;
                ensure!(
                    current.state.is_active(),
                    "agent task `{task_id}` became {} before the message could start",
                    current.status()
                );
            }
        }
        let current = self.get(task_id).await?;
        Ok(json!({
            "task_id": task_id,
            "name": current.name,
            "status": current.status(),
            "message_id": input_id,
            "requested_mode": mode.as_str(),
            "accepted_as": accepted_as,
        }))
    }

    pub async fn list_agents(&self) -> Result<Vec<super::BackgroundTaskRecord>> {
        Ok(self
            .select(&[])
            .await?
            .into_iter()
            .filter(|record| record.kind == "agent")
            .collect())
    }

    pub async fn stop(&self, task_id: &str) -> Result<super::BackgroundTaskRecord> {
        let current = self.get(task_id).await?;
        if current.state.is_terminal() {
            if current.state == BackgroundTaskState::Cancelled {
                self.cancel_child_run_if_active(&current).await?;
            }
            return Ok(current);
        }
        if current.kind == "agent" {
            return self.stop_agent(current).await;
        }
        let reason = "stopped by parent agent".to_owned();
        let record = self.cancel(task_id, reason).await?;
        if record.state != BackgroundTaskState::Cancelled {
            return Ok(record);
        }
        if let Some(tracked) = self.take_handle(task_id) {
            tracked.abort();
            tracked.wait().await;
        }
        if self.cancel_child_run_if_active(&record).await?
            && let Some(child_run_id) = &record.child_run_id
        {
            self.events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::SubagentActivityStopped {
                        child_run_id: child_run_id.clone(),
                    },
                ))
                .await?;
        }
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::BackgroundTaskCancelled {
                    task_id: record.id.clone(),
                    name: record.name.clone(),
                },
            ))
            .await?;
        Ok(record)
    }

    pub(super) async fn stop_agent(
        &self,
        current: super::BackgroundTaskRecord,
    ) -> Result<super::BackgroundTaskRecord> {
        ensure!(
            current.kind == "agent",
            "task `{}` is not an agent",
            current.id
        );
        if current.state == BackgroundTaskState::Closed {
            return Ok(current);
        }
        if let Some(tracked) = self.take_handle(&current.id) {
            tracked.abort();
            // The child loop transfers its run lease to cancellation cleanup.
            // Wait for that exact cleanup before making the same child reusable.
            tracked.wait().await;
        }
        let child_run_id = current
            .child_run_id
            .clone()
            .context("agent task is missing child_run_id")?;
        let record = self
            .interrupt_agent_activity(
                &current.id,
                "agent activity was stopped by the parent after its last complete checkpoint",
                "The parent stopped the previous agent activity after the last complete checkpoint. Any uncommitted tool side effects may still have occurred. Inspect state before continuing.",
            )
            .await?;
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::SubagentActivityStopped { child_run_id },
            ))
            .await;
        Ok(record)
    }

    pub async fn close(&self, task_id: &str) -> Result<super::BackgroundTaskRecord> {
        let mut records = self.records.lock().await;
        let current = records
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))?;
        ensure!(current.kind == "agent", "task `{task_id}` is not an agent");
        if current.state == BackgroundTaskState::Closed {
            return Ok(current);
        }
        ensure!(
            current.state == BackgroundTaskState::Idle,
            "agent task `{task_id}` must be idle before it can be closed"
        );
        let child_run_id = current
            .child_run_id
            .clone()
            .context("agent task is missing child_run_id")?;
        self.store
            .update_state(&child_run_id, RunState::Closed)
            .await?;
        let mut record = current;
        record.state = BackgroundTaskState::Closed;
        record.paused = false;
        record.pending_followups.clear();
        self.persist(&record).await?;
        records.insert(task_id.to_owned(), record.clone());
        drop(records);
        self.signal_activity();
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::SubagentClosed { child_run_id },
            ))
            .await;
        Ok(record)
    }

    pub(super) async fn cancel_child_run_if_active(
        &self,
        record: &super::BackgroundTaskRecord,
    ) -> Result<bool> {
        let Some(child_run_id) = &record.child_run_id else {
            return Ok(false);
        };
        let path = self.store.paths(child_run_id).metadata;
        if !tokio::fs::try_exists(path).await? {
            return Ok(false);
        }
        let child = self.store.load_run(child_run_id).await?;
        if !matches!(child.state, RunState::Queued | RunState::Running) {
            return Ok(false);
        }
        self.store
            .update_state(child_run_id, RunState::Cancelled)
            .await?;
        Ok(true)
    }
}
