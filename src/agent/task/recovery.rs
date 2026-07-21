use std::sync::Arc;

use anyhow::{Context, Result};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    storage::RunLease,
};

use super::{BackgroundTaskRecord, TaskManager, TaskManagerConfig, TaskRecordStore};

/// A child run which still needs to be resumed after its parent process was
/// restarted. `TaskManager` reconciles every other recoverable state itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverableSubagent {
    pub task_id: String,
    pub child_run_id: String,
    pub prompt: String,
}

/// Cancellation backstop owned by the agent loop. Normal completion disarms
/// it; dropping the loop future asks the task manager to abort and settle every
/// descendant instead of detaching their Tokio tasks.
#[must_use = "the guard must live for the full agent loop"]
pub(crate) struct TaskCancellationGuard {
    manager: Option<Arc<TaskManager>>,
    lease: Option<RunLease>,
}

impl TaskCancellationGuard {
    pub(crate) fn disarm(&mut self) {
        self.manager = None;
        self.lease = None;
    }
}

impl Drop for TaskCancellationGuard {
    fn drop(&mut self) {
        let Some(manager) = self.manager.take() else {
            return;
        };
        let lease = self.lease.take();
        let handles = manager.abort_handles();
        // Cancellation can drop an agent-loop future, so the guard cannot
        // await cleanup itself. Abort descendants synchronously, then finish
        // their durable cancelled states on the current runtime. A process
        // crash still falls back to restart reconciliation.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _lease = lease;
                manager
                    .settle_aborted(handles, "owning agent run was cancelled")
                    .await;
            });
        }
    }
}

impl TaskManager {
    /// Load durable task coordination without reconciling or mutating it. The
    /// runner validates the frozen capability schema before reconciliation.
    pub async fn load_for_resume(config: TaskManagerConfig) -> Result<Arc<Self>> {
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
                crate::model::MessageContent::BackgroundTask {
                    task_id,
                    status: Some(_),
                    ..
                } => Some(task_id),
                _ => None,
            })
            .collect();
        let manager = Self::from_config(config, records, delivered);
        Ok(manager)
    }

    #[cfg(test)]
    pub async fn restore(
        config: TaskManagerConfig,
    ) -> Result<(Arc<Self>, Vec<RecoverableSubagent>)> {
        let manager = Self::load_for_resume(config).await?;
        let recoverable = manager.reconcile_after_restart().await?;
        Ok((manager, recoverable))
    }

    pub(crate) fn cancellation_guard(self: &Arc<Self>, lease: RunLease) -> TaskCancellationGuard {
        TaskCancellationGuard {
            manager: Some(self.clone()),
            lease: Some(lease),
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
        let child_remaining_delegation_depth = task
            .child_remaining_delegation_depth
            .context("agent task is missing child delegation depth")?;
        let expected_profile = if child_remaining_delegation_depth > 0 {
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
            child.remaining_delegation_depth == child_remaining_delegation_depth,
            "child run `{child_run_id}` has remaining delegation depth {}, expected {}",
            child.remaining_delegation_depth,
            child_remaining_delegation_depth
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

    fn abort_handles(&self) -> Vec<(String, tokio::task::JoinHandle<()>)> {
        let handles = self.take_handles();
        for handle in handles.values() {
            handle.abort();
        }
        handles.into_iter().collect()
    }

    async fn settle_aborted(
        &self,
        handles: Vec<(String, tokio::task::JoinHandle<()>)>,
        reason: &str,
    ) {
        for (_, handle) in handles {
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
                    .update_state(child_run_id, crate::storage::RunState::Cancelled)
                    .await;
            }
            if self.cancel(&record.id, reason.to_owned()).await.is_ok() {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskCancelled {
                            task_id: record.id,
                            name: record.name,
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
                if record.state == super::BackgroundTaskState::Cancelled {
                    self.cancel_child_run_if_active(&record).await?;
                }
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
                    crate::storage::RunState::Cancelled => {
                        self.cancel(&record.id, "child run was cancelled".to_owned())
                            .await?;
                        continue;
                    }
                    crate::storage::RunState::Queued | crate::storage::RunState::Running => {}
                }
            }
            recoverable.push(RecoverableSubagent {
                task_id: record.id,
                child_run_id,
                prompt: record.prompt.context("agent task is missing prompt")?,
            });
        }
        Ok(recoverable)
    }
}
