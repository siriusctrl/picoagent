use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    model::openai_chat::project_chat_message,
    storage::RunState,
};

use super::{BackgroundTaskState, TaskManager};

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
            "status": record.status(),
            "messages": messages,
            "has_earlier": has_earlier,
            "next_before_seq": next_before_seq,
        }))
    }

    pub async fn steer(&self, task_id: &str, message: String) -> Result<Value> {
        ensure!(
            !message.trim().is_empty(),
            "steering message must not be empty"
        );
        let records = self.records.lock().await;
        let record = records
            .get(task_id)
            .cloned()
            .with_context(|| format!("unknown background task `{task_id}`"))?;
        ensure!(record.kind == "agent", "task `{task_id}` is not an agent");
        ensure!(
            !record.state.is_terminal(),
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
                matches!(child.state, RunState::Queued | RunState::Running),
                "child run `{child_run_id}` is already {:?}",
                child.state
            );
        }
        let input_id = self.store.enqueue_user_input(child_run_id, message).await?;
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::SubagentSteered {
                    task_id: task_id.to_owned(),
                    child_run_id: child_run_id.to_owned(),
                    input_id: input_id.clone(),
                },
            ))
            .await;
        drop(records);
        Ok(json!({
            "task_id": task_id,
            "status": record.status(),
        }))
    }

    pub async fn stop(&self, task_id: &str) -> Result<super::BackgroundTaskRecord> {
        let current = self.get(task_id).await?;
        if current.state.is_terminal() {
            if current.state == BackgroundTaskState::Cancelled {
                self.cancel_child_run_if_active(&current).await?;
            }
            return Ok(current);
        }
        let reason = "stopped by parent agent".to_owned();
        let record = self.cancel(task_id, reason).await?;
        if record.state != BackgroundTaskState::Cancelled {
            return Ok(record);
        }
        if let Some(handle) = self.take_handle(task_id) {
            handle.abort();
            let _ = handle.await;
        }
        if self.cancel_child_run_if_active(&record).await?
            && let Some(child_run_id) = &record.child_run_id
        {
            self.events
                .emit(&RuntimeEvent::new(
                    &self.parent_run_id,
                    RuntimeEventKind::SubagentCancelled {
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
