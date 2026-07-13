use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use ulid::Ulid;

use crate::{
    events::{RuntimeEvent, RuntimeEventKind},
    storage::RunState,
    tools::{RawToolOutput, ToolContext},
};

use super::{BackgroundTaskRecord, TaskManager};
use crate::agent::runner::RunRequest;

impl TaskManager {
    pub async fn spawn_tool(
        self: &Arc<Self>,
        name: String,
        arguments: Value,
        timeout_seconds: Option<u64>,
    ) -> Result<BackgroundTaskRecord> {
        let tool = self
            .tools
            .get(&name)
            .with_context(|| format!("unknown or non-spawnable tool `{name}`"))?;
        let task_id = self.create_task("tool", name.clone(), None).await?;
        let timeout = self.execution_timeout(timeout_seconds);
        let manager = self.clone();
        let task_id_for_future = task_id.clone();
        let handle = tokio::spawn(async move {
            let task_id = task_id_for_future;
            let permit = match manager.slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    manager.finish_failed(&task_id, &name, error.into()).await;
                    return;
                }
            };
            if let Err(error) = manager.set_running(&task_id).await {
                manager.finish_failed(&task_id, &name, error).await;
                return;
            }
            let _ = manager
                .events
                .emit(&RuntimeEvent::new(
                    &manager.parent_run_id,
                    RuntimeEventKind::BackgroundTaskStarted {
                        task_id: task_id.clone(),
                        name: name.clone(),
                    },
                ))
                .await;
            let context = ToolContext {
                run_id: manager.parent_run_id.clone(),
                call_id: format!("background-{task_id}"),
                workspace: manager.workspace.clone(),
            };
            let arguments = match manager
                .start_tool_lifecycle(&context, &name, arguments)
                .await
            {
                Ok(arguments) => arguments,
                Err(error) => {
                    manager.finish_failed(&task_id, &name, error).await;
                    return;
                }
            };
            let outcome =
                tokio::time::timeout(timeout, tool.execute(context.clone(), arguments)).await;
            drop(permit);
            match outcome {
                Ok(Ok(raw)) => match manager.persist_output(&context, raw).await {
                    Ok(output) => {
                        if let Err(error) = manager
                            .finish_tool_lifecycle(&context, &name, Some(&output), false)
                            .await
                        {
                            manager.finish_failed(&task_id, &name, error).await;
                        } else if let Err(error) =
                            manager.complete(&task_id, output.model_content()).await
                        {
                            manager.finish_failed(&task_id, &name, error).await;
                        } else {
                            let _ = manager
                                .events
                                .emit(&RuntimeEvent::new(
                                    &manager.parent_run_id,
                                    RuntimeEventKind::BackgroundTaskCompleted { task_id, name },
                                ))
                                .await;
                        }
                    }
                    Err(error) => manager.finish_failed(&task_id, &name, error).await,
                },
                Ok(Err(error)) => {
                    let _ = manager
                        .finish_tool_lifecycle(&context, &name, None, true)
                        .await;
                    manager.finish_failed(&task_id, &name, error).await;
                }
                Err(_) => {
                    let _ = manager
                        .finish_tool_lifecycle(&context, &name, None, true)
                        .await;
                    match manager.time_out(&task_id).await {
                        Ok(_) => {
                            let _ = manager
                                .events
                                .emit(&RuntimeEvent::new(
                                    &manager.parent_run_id,
                                    RuntimeEventKind::BackgroundTaskTimedOut { task_id, name },
                                ))
                                .await;
                        }
                        Err(error) => manager.finish_failed(&task_id, &name, error).await,
                    }
                }
            }
        });
        self.track(handle).await;
        self.get(&task_id).await
    }

    pub async fn spawn_agent(
        self: &Arc<Self>,
        profile: String,
        prompt: String,
        timeout_seconds: Option<u64>,
    ) -> Result<BackgroundTaskRecord> {
        if profile != "general-task" {
            bail!("unknown agent profile `{profile}`; expected `general-task`");
        }
        if prompt.trim().is_empty() {
            bail!("agent prompt must not be empty");
        }
        let child_run_id = Ulid::new().to_string();
        let task_id = self
            .create_task("agent", profile.clone(), Some(child_run_id.clone()))
            .await?;
        let timeout = self.execution_timeout(timeout_seconds);
        let manager = self.clone();
        let task_id_for_future = task_id.clone();
        let handle = tokio::spawn(async move {
            let task_id = task_id_for_future;
            let permit = match manager.slots.clone().acquire_owned().await {
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
            let request = RunRequest {
                prompt,
                parent_run_id: Some(manager.parent_run_id.clone()),
                depth: manager.parent_depth + 1,
                additional_instructions: Some(
                    "You are a focused general-task subagent. Complete the assigned task and return a concise result to the parent agent.".to_owned(),
                ),
                tool_allowlist: None,
                use_general_task_profile: true,
            };
            let outcome = tokio::time::timeout(
                timeout,
                manager.runner.run_with_id(request, child_run_id.clone()),
            )
            .await;
            drop(permit);
            match outcome {
                Ok(Ok(result)) => {
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
                            if let Err(error) =
                                manager.complete(&task_id, output.model_content()).await
                            {
                                manager.finish_failed(&task_id, &profile, error).await;
                            } else {
                                let _ = manager
                                    .events
                                    .emit(&RuntimeEvent::new(
                                        &manager.parent_run_id,
                                        RuntimeEventKind::BackgroundTaskCompleted {
                                            task_id,
                                            name: profile,
                                        },
                                    ))
                                    .await;
                                let _ = manager
                                    .events
                                    .emit(&RuntimeEvent::new(
                                        &manager.parent_run_id,
                                        RuntimeEventKind::SubagentCompleted { child_run_id },
                                    ))
                                    .await;
                            }
                        }
                        Err(error) => manager.finish_failed(&task_id, &profile, error).await,
                    }
                }
                Ok(Err(error)) => {
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
                Err(_) => {
                    let _ = manager
                        .store
                        .update_state(&child_run_id, RunState::Failed)
                        .await;
                    match manager.time_out(&task_id).await {
                        Ok(_) => {
                            let _ = manager
                                .events
                                .emit(&RuntimeEvent::new(
                                    &manager.parent_run_id,
                                    RuntimeEventKind::BackgroundTaskTimedOut {
                                        task_id,
                                        name: profile,
                                    },
                                ))
                                .await;
                        }
                        Err(error) => manager.finish_failed(&task_id, &profile, error).await,
                    }
                    let _ = manager
                        .events
                        .emit(&RuntimeEvent::new(
                            &manager.parent_run_id,
                            RuntimeEventKind::SubagentFailed {
                                child_run_id,
                                error: "background agent timed out".to_owned(),
                            },
                        ))
                        .await;
                }
            }
        });
        self.track(handle).await;
        self.get(&task_id).await
    }
}
