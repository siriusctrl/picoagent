use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use serde_json::Value;
use ulid::Ulid;

use crate::{
    agent::tool_execution::{ToolExecutionMode, ToolExecutionOutcome},
    events::{RuntimeEvent, RuntimeEventKind},
    model::ToolCall,
    storage::{RunLeaseBusy, RunState},
    tools::{RawToolOutput, ToolContext},
};

use super::{BackgroundTaskRecord, GENERAL_TASK_INSTRUCTIONS, TaskManager};
use crate::agent::runner::RunRequest;

impl TaskManager {
    pub async fn spawn_tool(
        self: &Arc<Self>,
        name: String,
        arguments: Value,
        timeout_seconds: Option<u64>,
    ) -> Result<BackgroundTaskRecord> {
        if self.tools.get(&name).is_none() {
            bail!("unknown or non-spawnable tool `{name}`")
        }
        let timeout = self.execution_timeout(timeout_seconds);
        let deadline = tokio::time::Instant::now() + timeout;
        let task_id = self
            .create_tool_task(name.clone(), timeout.as_secs())
            .await?;
        let manager = self.clone();
        let task_id_for_future = task_id.clone();
        let handle =
            tokio::spawn(async move {
                let task_id = task_id_for_future;
                let permit =
                    match tokio::time::timeout_at(deadline, manager.slots.clone().acquire_owned())
                        .await
                    {
                        Ok(Ok(permit)) => permit,
                        Err(_) => {
                            manager.finish_timed_out(&task_id, &name).await;
                            return;
                        }
                        Ok(Err(error)) => {
                            manager.finish_failed(&task_id, &name, error.into()).await;
                            return;
                        }
                    };
                let outcome = tokio::time::timeout_at(deadline, async {
                    manager.set_running(&task_id).await?;
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
                    let call = ToolCall {
                        id: format!("background-{task_id}"),
                        name: name.clone(),
                        arguments,
                    };
                    manager
                        .tool_executor()
                        .execute(
                            call,
                            deadline.saturating_duration_since(tokio::time::Instant::now()),
                            ToolExecutionMode::Background,
                        )
                        .await
                })
                .await;
                drop(permit);
                match outcome {
                    Ok(Ok(ToolExecutionOutcome::Completed(output))) => {
                        if let Err(error) = manager.complete(&task_id, *output).await {
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
                    Ok(Ok(ToolExecutionOutcome::Failed(error))) | Ok(Err(error)) => {
                        manager.finish_failed(&task_id, &name, error).await;
                    }
                    Ok(Ok(ToolExecutionOutcome::TimedOut)) | Err(_) => {
                        manager.finish_timed_out(&task_id, &name).await
                    }
                }
            });
        self.track(handle);
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
        let timeout = self.execution_timeout(timeout_seconds);
        let deadline = tokio::time::Instant::now() + timeout;
        let task_id = self
            .create_agent_task(
                profile.clone(),
                child_run_id.clone(),
                prompt.clone(),
                timeout.as_secs(),
            )
            .await?;
        let handle =
            self.launch_agent_task(task_id.clone(), profile, child_run_id, prompt, deadline);
        self.track(handle);
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
            task.task_id,
            record.name,
            task.child_run_id,
            task.prompt,
            tokio::time::Instant::now()
                + std::time::Duration::from_secs(task.timeout_seconds.max(1)),
        );
        self.track(handle);
        Ok(())
    }

    fn launch_agent_task(
        self: &Arc<Self>,
        task_id: String,
        profile: String,
        child_run_id: String,
        prompt: String,
        deadline: tokio::time::Instant,
    ) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        tokio::spawn(async move {
            let permit = match tokio::time::timeout_at(
                deadline,
                manager.slots.clone().acquire_owned(),
            )
            .await
            {
                Ok(Ok(permit)) => permit,
                Err(_) => {
                    manager.finish_timed_out(&task_id, &profile).await;
                    return;
                }
                Ok(Err(error)) => {
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
                GENERAL_TASK_INSTRUCTIONS.trim().to_owned(),
                manager.child_can_delegate,
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
            let outcome = tokio::time::timeout_at(deadline, async {
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
            })
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
                            if let Err(error) = manager.complete(&task_id, output).await {
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
                    manager.finish_timed_out(&task_id, &profile).await;
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
        })
    }
}
