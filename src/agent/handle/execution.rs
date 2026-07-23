use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use ulid::Ulid;

use crate::{
    agent::runner::RunRequest,
    events::{RuntimeEvent, RuntimeEventKind},
    prompts::agent_prompts,
    tools::{RawToolOutput, ToolContext},
};

use super::{
    super::tool_execution::ToolExecutionFuture, HandleKind, HandleOutput, HandleSnapshot,
    HandleState, RuntimeHandleManager,
};

pub(crate) struct PreparedToolPromotion {
    handle: String,
    name: String,
    call_id: String,
    promotion_ready: tokio::sync::oneshot::Sender<()>,
}

impl RuntimeHandleManager {
    pub async fn delegate(
        self: &Arc<Self>,
        name: String,
        prompt: String,
    ) -> Result<HandleSnapshot> {
        ensure!(
            self.remaining_delegation_depth > 0,
            "delegate is unavailable because remaining delegation depth is 0"
        );
        if name.trim().is_empty() {
            bail!("agent name must not be empty");
        }
        if prompt.trim().is_empty() {
            bail!("agent prompt must not be empty");
        }
        let handle = Ulid::new().to_string();
        let child_request = RunRequest::general_task(
            name.clone(),
            prompt,
            self.parent_run_id.clone(),
            self.parent_depth + 1,
            agent_prompts().general_task.clone(),
            self.remaining_delegation_depth.saturating_sub(1),
        );
        self.runner.prepare_run(&child_request, &handle).await?;
        self.insert_agent(handle.clone(), name).await?;
        let generation = self.queue_new_agent_activity(&handle).await?;
        self.launch_agent_activity(handle.clone(), generation);
        self.snapshot_for_handle(&handle).await
    }

    /// Continue a direct tool future after its foreground window elapsed. The
    /// exact future is preserved; only its process-local address changes.
    pub(crate) async fn prepare_tool_promotion(
        self: &Arc<Self>,
        name: String,
        call_id: String,
        execution: ToolExecutionFuture,
    ) -> Result<PreparedToolPromotion> {
        let handle = format!("j_{}", Ulid::new());
        self.insert_tool(handle.clone(), name.clone()).await?;
        let (promotion_ready, wait_for_promotion) = tokio::sync::oneshot::channel();
        let manager = self.clone();
        let handle_for_future = handle.clone();
        let name_for_future = name.clone();
        let call_id_for_future = call_id.clone();
        let task = tokio::spawn(async move {
            let outcome = execution.await;
            let _ = wait_for_promotion.await;
            manager
                .finish_tool_job(
                    &handle_for_future,
                    &name_for_future,
                    &call_id_for_future,
                    outcome,
                )
                .await;
        });
        self.track(handle.clone(), 0, task, None);
        Ok(PreparedToolPromotion {
            handle,
            name,
            call_id,
            promotion_ready,
        })
    }

    pub(crate) async fn announce_tool_promotion(
        &self,
        promotion: PreparedToolPromotion,
    ) -> Result<(String, String)> {
        let PreparedToolPromotion {
            handle,
            name,
            call_id,
            promotion_ready,
        } = promotion;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::RuntimeHandleStarted {
                    handle: handle.clone(),
                    kind: HandleKind::Tool.as_str().to_owned(),
                    name: name.clone(),
                },
            ))
            .await?;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::ToolSentToBackground {
                    handle: handle.clone(),
                    name: name.clone(),
                    call_id,
                },
            ))
            .await?;
        let _ = promotion_ready.send(());
        Ok((handle, name))
    }

    pub(super) async fn activate_agent_if_pending(self: &Arc<Self>, handle: &str) -> Result<bool> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(handle)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime handle `{handle}`"))?;
        ensure!(
            record.kind == HandleKind::Agent,
            "runtime handle `{handle}` is a tool job, not an agent"
        );
        if record.state != HandleState::Idle || record.followups.is_empty() {
            return Ok(false);
        }
        for input in &record.followups {
            self.store
                .enqueue_user_input_with_id(handle, input.id.clone(), input.message.clone())
                .await?;
        }
        record.followups.clear();
        record.state = HandleState::Queued;
        record.generation = record.generation.saturating_add(1);
        let generation = record.generation;
        drop(records);
        self.signal_activity();
        self.launch_agent_activity(handle.to_owned(), generation);
        Ok(true)
    }

    async fn queue_new_agent_activity(&self, handle: &str) -> Result<u64> {
        let mut records = self.records.lock().await;
        let record = records
            .get_mut(handle)
            .ok_or_else(|| anyhow::anyhow!("unknown runtime handle `{handle}`"))?;
        ensure!(
            record.kind == HandleKind::Agent && record.state == HandleState::Idle,
            "agent handle `{handle}` is not idle"
        );
        record.state = HandleState::Queued;
        record.generation = record.generation.saturating_add(1);
        let generation = record.generation;
        drop(records);
        self.signal_activity();
        Ok(generation)
    }

    pub(super) fn launch_agent_activity(self: &Arc<Self>, handle: String, generation: u64) {
        let manager = self.clone();
        let tracked_handle = handle.clone();
        let (start, wait_for_tracking) = tokio::sync::oneshot::channel();
        let (cleanup_done, wait_for_cleanup) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            if wait_for_tracking.await.is_err() {
                return;
            }
            let permit = match manager.subagent_slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    manager
                        .finish_agent_failure(&handle, generation, error.into())
                        .await;
                    return;
                }
            };
            let Some(name) = manager.begin_agent_activity(&handle, generation).await else {
                return;
            };
            manager.emit_agent_started(&handle, &name).await;
            let outcome = manager
                .runner
                .run_child_activity(handle.clone(), &manager.parent_run_id, cleanup_done)
                .await;
            drop(permit);
            match outcome {
                Ok(result) => {
                    let context = ToolContext {
                        run_id: manager.parent_run_id.clone(),
                        call_id: format!("agent-{handle}-{generation}"),
                        workspace: manager.workspace.clone(),
                    };
                    let raw = RawToolOutput::text(result.final_output);
                    match manager.artifacts.persist_output(&context, raw).await {
                        Ok(output) => {
                            if let Some(artifact) = &output.artifact {
                                let _ = manager
                                    .events
                                    .emit(&RuntimeEvent::new(
                                        &manager.parent_run_id,
                                        RuntimeEventKind::ArtifactCreated {
                                            call_id: context.call_id,
                                            path: artifact.path.clone(),
                                            bytes: artifact.bytes,
                                        },
                                    ))
                                    .await;
                            }
                            manager
                                .finish_agent_output(
                                    &handle,
                                    generation,
                                    HandleOutput {
                                        status: if output.is_error {
                                            HandleState::Failed
                                        } else {
                                            HandleState::Completed
                                        },
                                        content: output.model_content(),
                                        metadata: output.result_metadata(),
                                    },
                                )
                                .await;
                        }
                        Err(error) => {
                            manager
                                .finish_agent_failure(&handle, generation, error)
                                .await;
                        }
                    }
                }
                Err(error) => {
                    manager
                        .finish_agent_failure(&handle, generation, error)
                        .await;
                }
            }
        });
        self.track(tracked_handle, generation, task, Some(wait_for_cleanup));
        let _ = start.send(());
    }

    async fn begin_agent_activity(&self, handle: &str, generation: u64) -> Option<String> {
        let mut records = self.records.lock().await;
        let record = records.get_mut(handle)?;
        if record.kind != HandleKind::Agent
            || record.state != HandleState::Queued
            || record.generation != generation
        {
            return None;
        }
        record.state = HandleState::Running;
        let name = record.name.clone();
        drop(records);
        self.signal_activity();
        Some(name)
    }

    async fn emit_agent_started(&self, handle: &str, name: &str) {
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::RuntimeHandleStarted {
                    handle: handle.to_owned(),
                    kind: HandleKind::Agent.as_str().to_owned(),
                    name: name.to_owned(),
                },
            ))
            .await;
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::AgentActivityStarted {
                    handle: handle.to_owned(),
                },
            ))
            .await;
    }
}
