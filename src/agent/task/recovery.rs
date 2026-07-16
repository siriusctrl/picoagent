use std::sync::Arc;

use anyhow::{Context, Result};

use crate::events::{RuntimeEvent, RuntimeEventKind};

use super::{BackgroundTaskRecord, TaskManager, TaskManagerConfig, TaskRecordStore};

/// A child run which still needs to be resumed after its parent process was
/// restarted. `TaskManager` reconciles every other recoverable state itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableSubagent {
    pub task_id: String,
    pub child_run_id: String,
    pub prompt: String,
    pub timeout_seconds: u64,
}

/// Cancellation backstop owned by the agent loop. Normal completion disarms
/// it; dropping the loop future asks the task manager to abort and settle every
/// descendant instead of detaching their Tokio tasks.
#[must_use = "the guard must live for the full agent loop"]
pub(crate) struct TaskCancellationGuard {
    manager: Option<Arc<TaskManager>>,
}

impl TaskCancellationGuard {
    pub(crate) fn disarm(&mut self) {
        self.manager = None;
    }
}

impl Drop for TaskCancellationGuard {
    fn drop(&mut self) {
        let Some(manager) = self.manager.take() else {
            return;
        };
        // Drop cannot keep the parent run lease while awaiting durable state
        // changes. Abort only the in-memory work here; the next lease owner
        // reconciles the unchanged task records and resumes child runs.
        drop(manager.abort_handles());
    }
}

impl TaskManager {
    /// Reopen durable task coordination state for a resumed parent run.
    ///
    /// Completed child runs are folded into their parent task, in-flight tools
    /// become `interrupted` (and are never re-executed), and live child runs are
    /// returned to the caller for resumption by the shared `AgentRunner`.
    pub async fn restore(
        config: TaskManagerConfig,
    ) -> Result<(Arc<Self>, Vec<RecoverableSubagent>)> {
        let task_store = TaskRecordStore::new(
            config
                .store
                .paths(&config.parent_run_id)
                .directory
                .join("tasks"),
        );
        let records = task_store.load().await?;
        let delivered = config
            .store
            .load_messages(&config.parent_run_id)
            .await?
            .into_iter()
            .flat_map(|message| message.content)
            .filter_map(|content| match content {
                crate::model::MessageContent::BackgroundTaskResult { task_id, .. } => Some(task_id),
                _ => None,
            })
            .collect();
        let manager = Self::from_config(config, records, delivered);
        manager.restore_undelivered_preview_budget().await;
        let recoverable = manager.reconcile_after_restart().await?;
        Ok((manager, recoverable))
    }

    async fn restore_undelivered_preview_budget(&self) {
        let delivered = self.delivered.lock().await.clone();
        let used = self
            .records
            .lock()
            .await
            .values()
            .filter(|record| !delivered.contains(&record.id))
            .map(|record| record.result_metadata().preview_bytes)
            .fold(0_usize, usize::saturating_add);
        let mut remaining = self.preview_budget.lock().await;
        *remaining = remaining.saturating_sub(used);
    }

    pub(crate) fn cancellation_guard(self: &Arc<Self>) -> TaskCancellationGuard {
        TaskCancellationGuard {
            manager: Some(self.clone()),
        }
    }

    pub(super) fn validate_child_run(
        &self,
        task: &BackgroundTaskRecord,
        child: &crate::storage::RunRecord,
    ) -> Result<()> {
        let child_run_id = task
            .child_run_id
            .as_deref()
            .context("agent task is missing child_run_id")?;
        let prompt = task
            .prompt
            .as_deref()
            .context("agent task is missing prompt")?;
        anyhow::ensure!(
            child.id == child_run_id,
            "agent task `{}` references child `{child_run_id}` but its run metadata declares `{}`",
            task.id,
            child.id
        );
        anyhow::ensure!(
            child.parent_run_id.as_deref() == Some(self.parent_run_id.as_str()),
            "child run `{child_run_id}` does not belong to parent `{}`",
            self.parent_run_id
        );
        anyhow::ensure!(
            child.depth == self.parent_depth.saturating_add(1),
            "child run `{child_run_id}` has depth {}, expected {}",
            child.depth,
            self.parent_depth.saturating_add(1)
        );
        let expected_profile = if self.child_can_delegate {
            "general_task_delegating"
        } else {
            "general_task_leaf"
        };
        anyhow::ensure!(
            child.profile == expected_profile,
            "child run `{child_run_id}` has profile `{}`, expected `{expected_profile}`",
            child.profile
        );
        anyhow::ensure!(
            child.prompt == prompt,
            "child run `{child_run_id}` prompt does not match task `{}`",
            task.id
        );
        Ok(())
    }

    pub async fn abort_and_settle(&self, reason: &str) {
        let handles = self.abort_handles();
        self.settle_aborted(handles, reason).await;
    }

    fn abort_handles(&self) -> Vec<tokio::task::JoinHandle<()>> {
        let handles = self.take_handles();
        for handle in &handles {
            handle.abort();
        }
        handles
    }

    async fn settle_aborted(&self, handles: Vec<tokio::task::JoinHandle<()>>, reason: &str) {
        for handle in handles {
            let _ = handle.await;
        }
        let pending = self
            .select(&[])
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|record| !record.state.is_terminal())
            .collect::<Vec<_>>();
        for record in pending {
            if let Some(child_run_id) = &record.child_run_id
                && let Ok(run) = self.store.load_run(child_run_id).await
                && matches!(
                    run.state,
                    crate::storage::RunState::Queued | crate::storage::RunState::Running
                )
            {
                let _ = self
                    .store
                    .update_state(child_run_id, crate::storage::RunState::Failed)
                    .await;
            }
            if self.cancel(&record.id, reason.to_owned()).await.is_ok() {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskFailed {
                            task_id: record.id,
                            name: record.name,
                            error: reason.to_owned(),
                        },
                    ))
                    .await;
            }
        }
    }

    pub async fn reconcile_after_restart(&self) -> Result<Vec<RecoverableSubagent>> {
        let records = self.select(&[]).await?;
        let mut recoverable = Vec::new();
        for record in records {
            if record.state.is_terminal() {
                continue;
            }
            if record.kind == "tool" {
                self.interrupt(
                    &record.id,
                    "picoagent stopped while the tool was running; do not retry without checking its side effects"
                        .to_owned(),
                )
                .await?;
                continue;
            }
            let child_run_id = record
                .child_run_id
                .clone()
                .context("agent task is missing child_run_id")?;
            let child_paths = self.store.paths(&child_run_id);
            let mut live_child = false;
            if tokio::fs::try_exists(&child_paths.metadata).await? {
                let child = self.store.load_run(&child_run_id).await?;
                self.validate_child_run(&record, &child)?;
                match child.state {
                    crate::storage::RunState::Completed => {
                        let result = tokio::fs::read_to_string(&child_paths.final_output)
                            .await
                            .with_context(|| {
                                format!(
                                    "read completed child output {}",
                                    child_paths.final_output.display()
                                )
                            })?;
                        let context = crate::tools::ToolContext {
                            run_id: self.parent_run_id.clone(),
                            call_id: format!("background-{}", record.id),
                            workspace: self.workspace.clone(),
                        };
                        let output = self
                            .persist_output(&context, crate::tools::RawToolOutput::text(result))
                            .await?;
                        self.complete(&record.id, output).await?;
                        continue;
                    }
                    crate::storage::RunState::Failed => {
                        self.fail(&record.id, "child run failed".to_owned()).await?;
                        continue;
                    }
                    crate::storage::RunState::Queued | crate::storage::RunState::Running => {
                        live_child = true;
                    }
                }
            }
            let elapsed = chrono::Utc::now()
                .signed_duration_since(record.created_at)
                .num_seconds()
                .max(0) as u64;
            if elapsed >= record.timeout_seconds {
                if live_child {
                    self.store
                        .update_state(&child_run_id, crate::storage::RunState::Failed)
                        .await?;
                }
                self.time_out(&record.id).await?;
                continue;
            }
            recoverable.push(RecoverableSubagent {
                task_id: record.id,
                child_run_id,
                prompt: record.prompt.context("agent task is missing prompt")?,
                timeout_seconds: record.timeout_seconds.saturating_sub(elapsed).max(1),
            });
        }
        Ok(recoverable)
    }
}
