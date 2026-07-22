use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result};

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    storage::RunLease,
};

use super::{BackgroundTaskRecord, TaskManager, TaskManagerConfig, TaskRecordStore};

/// Cancellation backstop owned by the agent loop. Normal completion disarms
/// it; dropping the loop future asks the task manager to abort and settle every
/// descendant instead of detaching their Tokio tasks.
#[must_use = "the guard must live for the full agent loop"]
pub(crate) struct TaskCancellationGuard {
    manager: Option<Arc<TaskManager>>,
    lease: Option<RunLease>,
    cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
}

impl TaskCancellationGuard {
    pub(crate) fn disarm(&mut self) {
        self.manager = None;
        self.lease = None;
        if let Some(cleanup_done) = self.cleanup_done.take() {
            let _ = cleanup_done.send(());
        }
    }
}

impl Drop for TaskCancellationGuard {
    fn drop(&mut self) {
        let Some(manager) = self.manager.take() else {
            return;
        };
        let lease = self.lease.take();
        let cleanup_done = self.cleanup_done.take();
        let handles = manager.abort_handles();
        // Cancellation can drop an agent-loop future, so the guard cannot
        // await cleanup itself. Abort descendants synchronously, then finish
        // their durable cancelled states on the current runtime. A process
        // crash still falls back to restart reconciliation.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                manager
                    .settle_aborted(handles, "owning agent run was cancelled")
                    .await;
                // Waking task_stop is the permission to reuse this exact run,
                // so release its execution lease before publishing completion.
                drop(lease);
                if let Some(cleanup_done) = cleanup_done {
                    let _ = cleanup_done.send(());
                }
            });
        }
    }
}

impl TaskManager {
    /// Load durable task coordination without reconciling or mutating it. The
    /// runner validates the frozen capability schema before reconciliation.
    pub async fn load_existing(config: TaskManagerConfig) -> Result<Arc<Self>> {
        let task_store = TaskRecordStore::new(
            config
                .store
                .paths(&config.parent_run_id)
                .directory
                .join("tasks"),
        );
        let mut records = task_store.load().await?;
        let reserved_task_ids = records.keys().cloned().collect::<BTreeSet<_>>();
        let trajectory = config.store.load_trajectory(&config.parent_run_id).await?;
        let committed_call_results = trajectory
            .iter()
            .flat_map(|record| {
                record
                    .message
                    .content
                    .iter()
                    .filter_map(move |content| match content {
                        crate::model::MessageContent::ToolResult { call_id, .. } => {
                            Some((call_id.as_str(), record.created_at))
                        }
                        _ => None,
                    })
            })
            .collect::<Vec<_>>();
        // A task file written by an uncommitted tool turn is an orphan. The
        // process tree is dead before resume, so it must not be restarted or
        // shown to the model. Keep only task starts acknowledged by a complete
        // parent checkpoint.
        records.retain(|_, record| {
            committed_call_results.iter().any(|(call_id, created_at)| {
                *call_id == record.origin_call_id && *created_at >= record.created_at
            })
        });
        let mut delivered = BTreeMap::new();
        for content in trajectory
            .into_iter()
            .flat_map(|record| record.message.content)
        {
            if let crate::model::MessageContent::BackgroundTask {
                task_id,
                output_seq: Some(output_seq),
                status: Some(_),
                ..
            } = content
            {
                delivered
                    .entry(task_id)
                    .and_modify(|seq: &mut u64| *seq = (*seq).max(output_seq))
                    .or_insert(output_seq);
            }
        }
        let manager = Self::from_config(config, records, delivered, reserved_task_ids);
        Ok(manager)
    }

    #[cfg(test)]
    pub async fn restore(config: TaskManagerConfig) -> Result<Arc<Self>> {
        let manager = Self::load_existing(config).await?;
        manager.reconcile_stale_tasks().await?;
        Ok(manager)
    }

    pub(crate) fn cancellation_guard(
        self: &Arc<Self>,
        lease: RunLease,
        cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> TaskCancellationGuard {
        TaskCancellationGuard {
            manager: Some(self.clone()),
            lease: Some(lease),
            cleanup_done,
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

    fn abort_handles(&self) -> Vec<(String, super::TrackedTask)> {
        let handles = self.take_handles();
        for tracked in handles.values() {
            tracked.abort();
        }
        handles.into_iter().collect()
    }

    async fn settle_aborted(&self, handles: Vec<(String, super::TrackedTask)>, reason: &str) {
        for (_, tracked) in handles {
            tracked.wait().await;
        }
        let pending = self
            .select(&[])
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|record| record.state.is_active())
            .collect::<Vec<_>>();
        for record in pending {
            if record.kind == "agent" {
                let _ = self.stop_agent(record).await;
                continue;
            }
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

    /// Settle persisted active records loaded into a manager which owns no
    /// corresponding in-memory handles. This is conservative for both a root
    /// process restart and later reuse of an inactive child runner.
    pub async fn reconcile_stale_tasks(self: &Arc<Self>) -> Result<()> {
        let records = self.select(&[]).await?;
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
                    "fiasco stopped while the tool was running; do not retry without checking its side effects"
                        .to_owned(),
                )
                .await?;
                continue;
            }
            let child_run_id = record
                .child_run_id
                .clone()
                .context("agent task is missing child_run_id")?;
            let child = self.store.load_run(&child_run_id).await.with_context(|| {
                format!(
                    "committed agent task `{}` is missing child run `{child_run_id}`",
                    record.id
                )
            })?;
            self.validate_child_run(&record, &child)?;
            if child.state == crate::storage::RunState::Closed {
                anyhow::ensure!(
                    record.state == super::BackgroundTaskState::Idle,
                    "agent task `{}` is {} while child `{child_run_id}` is closed",
                    record.id,
                    record.status()
                );
                self.update(&record.id, |record| {
                    record.state = super::BackgroundTaskState::Closed;
                    record.paused = false;
                    record.pending_followups.clear();
                })
                .await?;
                continue;
            }
            if record.state == super::BackgroundTaskState::Idle {
                if child.state != crate::storage::RunState::Idle {
                    self.store
                        .update_state(&child_run_id, crate::storage::RunState::Idle)
                        .await?;
                }
                continue;
            }
            self.interrupt_agent_activity(
                &record.id,
                "agent activity was interrupted when the fiasco process stopped; the agent thread remains available for an explicit task_send",
                "The previous agent activity was interrupted after its last complete checkpoint because the fiasco process stopped. Any uncommitted tool side effects may still have occurred. Inspect state before continuing.",
            )
            .await?;
        }
        Ok(())
    }
}
