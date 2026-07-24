use std::sync::Arc;

use anyhow::Result;

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    storage::RunLease,
    tools::{RawToolOutput, ToolContext},
};

use super::{HandleKind, HandleOutput, HandleSnapshot, HandleState, RuntimeHandleManager};

/// Keeps descendants and the owning run lease tied to the agent-loop future.
#[must_use = "the guard must live for the full agent loop"]
pub(crate) struct HandleCancellationGuard {
    manager: Option<Arc<RuntimeHandleManager>>,
    lease: Option<RunLease>,
    cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HandleCancellationGuard {
    pub(crate) fn disarm(&mut self) {
        self.manager = None;
        self.lease = None;
        if let Some(cleanup_done) = self.cleanup_done.take() {
            let _ = cleanup_done.send(());
        }
    }
}

impl Drop for HandleCancellationGuard {
    fn drop(&mut self) {
        let Some(manager) = self.manager.take() else {
            return;
        };
        let lease = self.lease.take();
        let cleanup_done = self.cleanup_done.take();
        let executions = manager.abort_executions();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                manager.settle_aborted(executions).await;
                drop(lease);
                if let Some(cleanup_done) = cleanup_done {
                    let _ = cleanup_done.send(());
                }
            });
        }
    }
}

impl RuntimeHandleManager {
    pub(crate) fn cancellation_guard(
        self: &Arc<Self>,
        lease: RunLease,
        cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> HandleCancellationGuard {
        HandleCancellationGuard {
            manager: Some(self.clone()),
            lease: Some(lease),
            cleanup_done,
        }
    }

    pub async fn abort_and_settle(&self) {
        let executions = self.abort_executions();
        self.settle_aborted(executions).await;
    }

    fn abort_executions(&self) -> Vec<(String, super::TrackedExecution)> {
        let executions = self.take_executions();
        for tracked in executions.values() {
            tracked.abort();
        }
        executions.into_iter().collect()
    }

    async fn settle_aborted(&self, executions: Vec<(String, super::TrackedExecution)>) {
        for (_, tracked) in executions {
            tracked.wait().await;
        }
    }

    pub(super) async fn finish_tool_job(
        &self,
        handle: &str,
        name: &str,
        call_id: &str,
        outcome: Result<crate::artifact::ToolOutput>,
    ) {
        let output = match outcome {
            Ok(output) => HandleOutput {
                status: if output.is_error {
                    HandleState::Failed
                } else {
                    HandleState::Completed
                },
                content: output.model_content(),
                metadata: output.result_metadata(),
            },
            Err(error) => {
                let message = format!("{error:#}");
                let context = ToolContext {
                    run_id: self.parent_run_id.clone(),
                    call_id: call_id.to_owned(),
                    workspace: self.workspace.clone(),
                };
                let raw = RawToolOutput {
                    content: message.as_bytes().to_vec(),
                    source_path: None,
                    media_type: "text/plain; charset=utf-8".to_owned(),
                    is_error: true,
                    attach_to_model: false,
                };
                match self.artifacts.persist_output(&context, raw).await {
                    Ok(output) => HandleOutput {
                        status: HandleState::Failed,
                        content: output.model_content(),
                        metadata: output.result_metadata(),
                    },
                    Err(_) => HandleOutput {
                        status: HandleState::Failed,
                        content: message,
                        metadata: crate::artifact::ResultMetadata::empty(),
                    },
                }
            }
        };
        let status = output.status;
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(handle) else {
            return;
        };
        if record.kind != HandleKind::Tool || !record.state.is_active() {
            return;
        }
        record.state = status;
        record.outputs.push_back(output);
        drop(records);
        self.signal_activity();
        let kind = match status {
            HandleState::Completed => RuntimeEventKind::RuntimeHandleCompleted {
                handle: handle.to_owned(),
                kind: HandleKind::Tool.as_str().to_owned(),
                name: name.to_owned(),
            },
            _ => RuntimeEventKind::RuntimeHandleFailed {
                handle: handle.to_owned(),
                kind: HandleKind::Tool.as_str().to_owned(),
                name: name.to_owned(),
            },
        };
        let _ = self
            .events
            .emit(&RuntimeEvent::new(&self.parent_run_id, kind))
            .await;
    }

    pub(super) async fn finish_agent_output(
        self: &Arc<Self>,
        handle: &str,
        generation: u64,
        output: HandleOutput,
    ) {
        let status = output.status;
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(handle) else {
            return;
        };
        if record.kind != HandleKind::Agent
            || !record.state.is_active()
            || record.generation != generation
        {
            return;
        }
        let name = record.name.clone();
        record.state = HandleState::Idle;
        record.outputs.push_back(output);
        drop(records);
        self.signal_activity();
        let kind = if status == HandleState::Completed {
            RuntimeEventKind::RuntimeHandleCompleted {
                handle: handle.to_owned(),
                kind: HandleKind::Agent.as_str().to_owned(),
                name,
            }
        } else {
            RuntimeEventKind::RuntimeHandleFailed {
                handle: handle.to_owned(),
                kind: HandleKind::Agent.as_str().to_owned(),
                name,
            }
        };
        let _ = self
            .events
            .emit(&RuntimeEvent::new(&self.parent_run_id, kind))
            .await;
        let activity_kind = if status == HandleState::Completed {
            RuntimeEventKind::AgentActivityCompleted {
                handle: handle.to_owned(),
            }
        } else {
            RuntimeEventKind::AgentActivityFailed {
                handle: handle.to_owned(),
            }
        };
        let _ = self
            .events
            .emit(&RuntimeEvent::new(&self.parent_run_id, activity_kind))
            .await;
        if let Err(error) = self.activate_agent_if_pending(handle).await {
            tracing::error!(handle, error = %format!("{error:#}"), "activate queued agent follow-up");
        }
    }

    pub(super) async fn finish_agent_failure(
        self: &Arc<Self>,
        handle: &str,
        generation: u64,
        error: anyhow::Error,
    ) {
        let message = format!("{error:#}");
        let context = ToolContext {
            run_id: self.parent_run_id.clone(),
            call_id: format!("agent-{handle}-{generation}"),
            workspace: self.workspace.clone(),
        };
        let raw = RawToolOutput {
            content: message.as_bytes().to_vec(),
            source_path: None,
            media_type: "text/plain; charset=utf-8".to_owned(),
            is_error: true,
            attach_to_model: false,
        };
        let output = match self.artifacts.persist_output(&context, raw).await {
            Ok(output) => HandleOutput {
                status: HandleState::Failed,
                content: output.model_content(),
                metadata: output.result_metadata(),
            },
            Err(_) => HandleOutput {
                status: HandleState::Failed,
                content: message,
                metadata: crate::artifact::ResultMetadata::empty(),
            },
        };
        self.finish_agent_output(handle, generation, output).await;
    }

    pub(super) async fn stop_agent(&self, handle: &str) -> Result<HandleSnapshot> {
        let generation = {
            let records = self.records.lock().await;
            let record = records
                .get(handle)
                .ok_or_else(|| anyhow::anyhow!("unknown runtime handle `{handle}`"))?;
            if !record.state.is_active() {
                return Ok(record.snapshot(handle));
            }
            record.generation
        };
        let Some(tracked) = self.take_execution(handle, generation) else {
            return self.snapshot_for_handle(handle).await;
        };
        tracked.abort();
        tracked.wait().await;
        let (snapshot, stopped) = self.record_agent_stop(handle, generation).await?;
        if !stopped {
            return Ok(snapshot);
        }
        self.signal_activity();
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::RuntimeHandleStopped {
                    handle: handle.to_owned(),
                    kind: HandleKind::Agent.as_str().to_owned(),
                },
            ))
            .await?;
        Ok(snapshot)
    }

    pub(super) async fn record_agent_stop(
        &self,
        handle: &str,
        generation: u64,
    ) -> Result<(HandleSnapshot, bool)> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(handle)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime handle `{handle}`"))?;
        if !record.state.is_active() || record.generation != generation {
            return Ok((record.snapshot(handle), false));
        }
        self.store
            .enqueue_runtime_input_with_id(
                handle,
                format!("stopped-{}", ulid::Ulid::new()),
                "The parent stopped the previous agent activity. Its incomplete trailing tool turn was discarded, but tool side effects may still have occurred. Inspect state before continuing."
                    .to_owned(),
            )
            .await?;
        record.state = HandleState::Idle;
        record.outputs.push_back(HandleOutput {
            status: HandleState::Cancelled,
            content: "agent activity was stopped by the parent; any incomplete trailing tool turn was discarded"
                .to_owned(),
            metadata: crate::artifact::ResultMetadata::empty(),
        });
        let snapshot = record.snapshot(handle);
        Ok((snapshot, true))
    }

    pub(super) async fn stop_tool(&self, handle: &str) -> Result<HandleSnapshot> {
        {
            let records = self.records.lock().await;
            let record = records
                .get(handle)
                .ok_or_else(|| anyhow::anyhow!("unknown runtime handle `{handle}`"))?;
            if !record.state.is_active() {
                return Ok(record.snapshot(handle));
            }
        }
        let Some(tracked) = self.take_execution(handle, 0) else {
            return self.snapshot_for_handle(handle).await;
        };
        tracked.abort();
        tracked.wait().await;
        let (snapshot, stopped) = self.record_tool_stop(handle).await?;
        if !stopped {
            return Ok(snapshot);
        }
        self.signal_activity();
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::RuntimeHandleStopped {
                    handle: handle.to_owned(),
                    kind: HandleKind::Tool.as_str().to_owned(),
                },
            ))
            .await?;
        Ok(snapshot)
    }

    pub(super) async fn record_tool_stop(&self, handle: &str) -> Result<(HandleSnapshot, bool)> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(handle)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime handle `{handle}`"))?;
        if !record.state.is_active() {
            return Ok((record.snapshot(handle), false));
        }
        record.state = HandleState::Cancelled;
        record.outputs.push_back(HandleOutput {
            status: HandleState::Cancelled,
            content: "tool job was stopped by the parent agent".to_owned(),
            metadata: crate::artifact::ResultMetadata::empty(),
        });
        let snapshot = record.snapshot(handle);
        Ok((snapshot, true))
    }
}
