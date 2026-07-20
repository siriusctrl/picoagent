use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use ulid::Ulid;

use crate::{
    agent::{runner::RunRequest, tool_execution::ToolExecutionFuture},
    events::{RuntimeEvent, RuntimeEventKind},
    prompts::agent_prompts,
    storage::RunLeaseBusy,
    tools::{RawToolOutput, ToolContext},
};

use super::{BackgroundTaskRecord, TaskManager};

pub(crate) struct PreparedToolPromotion {
    task_id: String,
    name: String,
    call_id: String,
    promotion_ready: tokio::sync::oneshot::Sender<()>,
}

impl TaskManager {
    /// Continue a direct tool future after its foreground window elapsed. The
    /// future itself is preserved, so no work is restarted and no timeout is
    /// treated as a failure.
    pub(crate) async fn prepare_tool_promotion(
        self: &Arc<Self>,
        name: String,
        call_id: String,
        execution: ToolExecutionFuture,
    ) -> Result<PreparedToolPromotion> {
        let task_id = self.create_tool_task(name.clone(), call_id.clone()).await?;
        self.set_running(&task_id).await?;
        // `execution` has already been polled in the foreground and may be
        // suspended while holding a resource also needed by task events (for
        // example, the run event-log lock). Resume it before awaiting those
        // events, otherwise promotion can deadlock against its own future.
        // Delay the terminal task transition until the promotion events have
        // been committed so task lifecycle events remain ordered.
        let (promotion_ready, wait_for_promotion) = tokio::sync::oneshot::channel();
        let manager = self.clone();
        let task_id_for_future = task_id.clone();
        let name_for_future = name.clone();
        let handle = tokio::spawn(async move {
            let outcome = execution.await;
            let _ = wait_for_promotion.await;
            match outcome {
                Ok(output) => {
                    manager
                        .finish_tool_output(&task_id_for_future, &name_for_future, output)
                        .await;
                }
                Err(error) => {
                    manager
                        .finish_failed(&task_id_for_future, &name_for_future, error)
                        .await;
                }
            }
        });
        self.track(task_id.clone(), handle);
        Ok(PreparedToolPromotion {
            task_id,
            name,
            call_id,
            promotion_ready,
        })
    }

    pub(crate) async fn announce_tool_promotion(
        &self,
        promotion: PreparedToolPromotion,
    ) -> Result<String> {
        let PreparedToolPromotion {
            task_id,
            name,
            call_id,
            promotion_ready,
        } = promotion;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::BackgroundTaskStarted {
                    task_id: task_id.clone(),
                    name: name.clone(),
                },
            ))
            .await?;
        self.events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::BackgroundTaskSentToBackground {
                    task_id: task_id.clone(),
                    name: name.clone(),
                    call_id,
                },
            ))
            .await?;
        let _ = promotion_ready.send(());
        Ok(task_id)
    }

    async fn finish_tool_output(
        &self,
        task_id: &str,
        name: &str,
        output: crate::artifact::ToolOutput,
    ) {
        let state = if output.is_error {
            self.fail_with_output(
                task_id,
                format!("tool `{name}` returned an error result"),
                output,
            )
            .await
        } else {
            self.complete(task_id, output).await
        };
        match state {
            Ok(record) if record.state == super::BackgroundTaskState::Completed => {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskCompleted {
                            task_id: task_id.to_owned(),
                            name: name.to_owned(),
                        },
                    ))
                    .await;
            }
            Ok(record) if record.state == super::BackgroundTaskState::Failed => {
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::BackgroundTaskFailed {
                            task_id: task_id.to_owned(),
                            name: name.to_owned(),
                            error: record.error.unwrap_or_else(|| "tool failed".to_owned()),
                        },
                    ))
                    .await;
            }
            Ok(_) => {}
            Err(error) => self.finish_failed(task_id, name, error).await,
        }
    }

    pub async fn delegate(self: &Arc<Self>, prompt: String) -> Result<BackgroundTaskRecord> {
        if prompt.trim().is_empty() {
            bail!("agent prompt must not be empty");
        }
        let profile = "general-task".to_owned();
        let child_run_id = Ulid::new().to_string();
        let task_id = self
            .create_agent_task(profile.clone(), child_run_id.clone(), prompt.clone())
            .await?;
        let handle = self.launch_agent_task(
            task_id.clone(),
            profile,
            child_run_id,
            prompt,
            self.child_can_delegate,
        );
        self.track(task_id.clone(), handle);
        self.get(&task_id).await
    }

    pub async fn resume_agent_task(
        self: &Arc<Self>,
        task: super::RecoverableSubagent,
    ) -> Result<()> {
        let record = self.get(&task.task_id).await?;
        ensure!(
            record.kind == "agent",
            "task `{}` is not an agent",
            task.task_id
        );
        ensure!(
            !record.state.is_terminal(),
            "agent task `{}` is already terminal",
            task.task_id
        );
        ensure!(
            record.child_run_id.as_deref() == Some(task.child_run_id.as_str()),
            "agent task `{}` child run changed during recovery",
            task.task_id
        );
        let child_path = self.store.paths(&task.child_run_id).metadata;
        if tokio::fs::try_exists(&child_path).await? {
            let child = self.store.load_run(&task.child_run_id).await?;
            self.validate_child_run(&record, &child)?;
        }
        let handle = self.launch_agent_task(
            task.task_id.clone(),
            record.name,
            task.child_run_id,
            task.prompt,
            record
                .child_can_delegate
                .context("agent task is missing child capability")?,
        );
        self.track(task.task_id, handle);
        Ok(())
    }

    fn launch_agent_task(
        self: &Arc<Self>,
        task_id: String,
        profile: String,
        child_run_id: String,
        prompt: String,
        child_can_delegate: bool,
    ) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        tokio::spawn(async move {
            let permit = match manager.subagent_slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    manager
                        .finish_failed(&task_id, &profile, error.into())
                        .await;
                    return;
                }
            };
            if let Err(error) = manager.set_running(&task_id).await {
                manager.finish_failed(&task_id, &profile, error).await;
                return;
            }
            let _ = manager
                .events
                .emit(&RuntimeEvent::new(
                    &manager.parent_run_id,
                    RuntimeEventKind::BackgroundTaskStarted {
                        task_id: task_id.clone(),
                        name: profile.clone(),
                    },
                ))
                .await;
            let _ = manager
                .events
                .emit(&RuntimeEvent::new(
                    &manager.parent_run_id,
                    RuntimeEventKind::SubagentStarted {
                        child_run_id: child_run_id.clone(),
                        task: prompt.clone(),
                    },
                ))
                .await;
            let request = RunRequest::general_task(
                prompt,
                manager.parent_run_id.clone(),
                manager.parent_depth + 1,
                agent_prompts().general_task.clone(),
                child_can_delegate,
            );
            let child_exists =
                match tokio::fs::try_exists(manager.store.paths(&child_run_id).metadata).await {
                    Ok(child_exists) => child_exists,
                    Err(error) => {
                        manager
                            .finish_failed(&task_id, &profile, error.into())
                            .await;
                        return;
                    }
                };
            if child_exists {
                let validation = async {
                    let record = manager.get(&task_id).await?;
                    let child = manager.store.load_run(&child_run_id).await?;
                    manager.validate_child_run(&record, &child)
                }
                .await;
                if let Err(error) = validation {
                    manager.finish_failed(&task_id, &profile, error).await;
                    return;
                }
            }
            let outcome = async {
                if child_exists {
                    loop {
                        match manager
                            .runner
                            .resume_child(child_run_id.clone(), &manager.parent_run_id)
                            .await
                        {
                            Err(error) if error.downcast_ref::<RunLeaseBusy>().is_some() => {
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            }
                            result => break result,
                        }
                    }
                } else {
                    manager
                        .runner
                        .run_with_id(request, child_run_id.clone())
                        .await
                }
            }
            .await;
            drop(permit);
            match outcome {
                Ok(result) => {
                    let context = ToolContext {
                        run_id: manager.parent_run_id.clone(),
                        call_id: format!("background-{task_id}"),
                        workspace: manager.workspace.clone(),
                    };
                    let raw = RawToolOutput::text(result.final_output);
                    match manager.persist_output(&context, raw).await {
                        Ok(output) => {
                            if let Some(artifact) = &output.artifact {
                                let _ = manager
                                    .events
                                    .emit(&RuntimeEvent::new(
                                        &manager.parent_run_id,
                                        RuntimeEventKind::ArtifactCreated {
                                            call_id: context.call_id.clone(),
                                            path: artifact.path.clone(),
                                            bytes: artifact.bytes,
                                        },
                                    ))
                                    .await;
                            }
                            manager
                                .finish_agent_output(&task_id, &profile, &child_run_id, output)
                                .await;
                        }
                        Err(error) => manager.finish_failed(&task_id, &profile, error).await,
                    }
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    manager
                        .finish_failed(&task_id, &profile, anyhow::anyhow!(message.clone()))
                        .await;
                    let _ = manager
                        .events
                        .emit(&RuntimeEvent::new(
                            &manager.parent_run_id,
                            RuntimeEventKind::SubagentFailed {
                                child_run_id,
                                error: message,
                            },
                        ))
                        .await;
                }
            }
        })
    }
}
