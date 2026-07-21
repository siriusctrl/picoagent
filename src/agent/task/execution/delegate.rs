use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use ulid::Ulid;

use crate::{
    agent::{runner::RunRequest, types::DelegatedContext},
    events::{RuntimeEvent, RuntimeEventKind},
    model::{MessageContent, Role},
    prompts::agent_prompts,
    storage::{DelegateContext, RunLeaseBusy},
    tools::{RawToolOutput, ToolContext},
};

use super::super::{BackgroundTaskRecord, TaskManager};

impl TaskManager {
    pub async fn delegate(
        self: &Arc<Self>,
        name: String,
        prompt: String,
        delegate_context: DelegateContext,
        origin_call_id: &str,
    ) -> Result<BackgroundTaskRecord> {
        ensure!(
            self.remaining_delegation_depth > 0,
            "delegate is unavailable because remaining delegation depth is 0"
        );
        let name = name.trim().to_owned();
        if name.is_empty() {
            bail!("agent task name must not be empty");
        }
        if name.chars().any(char::is_control) {
            bail!("agent task name must not contain control characters");
        }
        if name.chars().count() > 64 {
            bail!("agent task name must be at most 64 characters");
        }
        if prompt.trim().is_empty() {
            bail!("agent prompt must not be empty");
        }
        let fork_parent_message_seq = match delegate_context {
            DelegateContext::Fresh => None,
            DelegateContext::Fork => Some(self.delegate_fork_boundary(origin_call_id).await?),
        };
        let child_run_id = Ulid::new().to_string();
        let task_id = self
            .create_agent_task(
                name.clone(),
                child_run_id,
                prompt,
                delegate_context,
                fork_parent_message_seq,
                origin_call_id.to_owned(),
            )
            .await?;
        let handle = self.launch_agent_task(task_id.clone());
        self.track(task_id.clone(), handle);
        self.get(&task_id).await
    }

    pub async fn resume_agent_task(
        self: &Arc<Self>,
        task: super::super::RecoverableSubagent,
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
        let handle = self.launch_agent_task(task.task_id.clone());
        self.track(task.task_id, handle);
        Ok(())
    }

    fn launch_agent_task(self: &Arc<Self>, task_id: String) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        tokio::spawn(async move {
            let record = match manager.get(&task_id).await {
                Ok(record) => record,
                Err(error) => {
                    tracing::error!(task_id, error = %format!("{error:#}"), "load agent task for launch");
                    return;
                }
            };
            let task_name = record.name;
            let child_run_id = record
                .child_run_id
                .expect("validated agent task must have a child run id");
            let prompt = record
                .prompt
                .expect("validated agent task must have a prompt");
            let child_remaining_delegation_depth = record
                .child_remaining_delegation_depth
                .expect("validated agent task must have child delegation depth");
            let delegate_context = record
                .delegate_context
                .expect("validated agent task must have a delegate context");
            let fork_parent_message_seq = record.fork_parent_message_seq;
            let permit = match manager.subagent_slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    manager
                        .finish_failed(&task_id, &task_name, error.into())
                        .await;
                    return;
                }
            };
            if let Err(error) = manager.set_running(&task_id).await {
                manager.finish_failed(&task_id, &task_name, error).await;
                return;
            }
            manager
                .emit_agent_started(&task_id, &task_name, &child_run_id, &prompt)
                .await;
            let child_exists =
                match tokio::fs::try_exists(manager.store.paths(&child_run_id).metadata).await {
                    Ok(child_exists) => child_exists,
                    Err(error) => {
                        manager
                            .finish_failed(&task_id, &task_name, error.into())
                            .await;
                        return;
                    }
                };
            let request = if child_exists {
                None
            } else {
                let model_override = if delegate_context == DelegateContext::Fork {
                    match manager.store.load_run(&manager.parent_run_id).await {
                        Ok(parent) => Some(parent.model),
                        Err(error) => {
                            manager.finish_failed(&task_id, &task_name, error).await;
                            return;
                        }
                    }
                } else {
                    None
                };
                Some(RunRequest::general_task(
                    prompt,
                    manager.parent_run_id.clone(),
                    manager.parent_depth + 1,
                    agent_prompts().general_task.clone(),
                    child_remaining_delegation_depth,
                    DelegatedContext {
                        mode: delegate_context,
                        fork_parent_message_seq,
                        model_override,
                    },
                ))
            };
            if child_exists {
                let validation = async {
                    let record = manager.get(&task_id).await?;
                    let child = manager.store.load_run(&child_run_id).await?;
                    manager.validate_child_run(&record, &child)
                }
                .await;
                if let Err(error) = validation {
                    manager.finish_failed(&task_id, &task_name, error).await;
                    return;
                }
            }
            let outcome = manager
                .run_agent_child(child_exists, request, &child_run_id)
                .await;
            drop(permit);
            manager
                .finish_agent_child(outcome, &task_id, &task_name, child_run_id)
                .await;
        })
    }

    async fn emit_agent_started(
        &self,
        task_id: &str,
        task_name: &str,
        child_run_id: &str,
        prompt: &str,
    ) {
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::BackgroundTaskStarted {
                    task_id: task_id.to_owned(),
                    name: task_name.to_owned(),
                },
            ))
            .await;
        let _ = self
            .events
            .emit(&RuntimeEvent::new(
                &self.parent_run_id,
                RuntimeEventKind::SubagentStarted {
                    child_run_id: child_run_id.to_owned(),
                    task: prompt.to_owned(),
                },
            ))
            .await;
    }

    async fn run_agent_child(
        &self,
        child_exists: bool,
        request: Option<RunRequest>,
        child_run_id: &str,
    ) -> Result<crate::agent::runner::RunResult> {
        if child_exists {
            loop {
                match self
                    .runner
                    .resume_child(child_run_id.to_owned(), &self.parent_run_id)
                    .await
                {
                    Err(error) if error.downcast_ref::<RunLeaseBusy>().is_some() => {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    result => break result,
                }
            }
        } else {
            self.runner
                .run_with_id(
                    request.expect("missing child run must have a run request"),
                    child_run_id.to_owned(),
                )
                .await
        }
    }

    async fn finish_agent_child(
        &self,
        outcome: Result<crate::agent::runner::RunResult>,
        task_id: &str,
        task_name: &str,
        child_run_id: String,
    ) {
        match outcome {
            Ok(result) => {
                let context = ToolContext {
                    run_id: self.parent_run_id.clone(),
                    call_id: format!("background-{task_id}"),
                    workspace: self.workspace.clone(),
                };
                let raw = RawToolOutput::text(result.final_output);
                match self.persist_output(&context, raw).await {
                    Ok(output) => {
                        if let Some(artifact) = &output.artifact {
                            let _ = self
                                .events
                                .emit(&RuntimeEvent::new(
                                    &self.parent_run_id,
                                    RuntimeEventKind::ArtifactCreated {
                                        call_id: context.call_id,
                                        path: artifact.path.clone(),
                                        bytes: artifact.bytes,
                                    },
                                ))
                                .await;
                        }
                        self.finish_agent_output(task_id, task_name, &child_run_id, output)
                            .await;
                    }
                    Err(error) => self.finish_failed(task_id, task_name, error).await,
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.finish_failed(task_id, task_name, anyhow::anyhow!(message.clone()))
                    .await;
                let _ = self
                    .events
                    .emit(&RuntimeEvent::new(
                        &self.parent_run_id,
                        RuntimeEventKind::SubagentFailed {
                            child_run_id,
                            error: message,
                        },
                    ))
                    .await;
            }
        }
    }

    async fn delegate_fork_boundary(&self, call_id: &str) -> Result<u64> {
        let trajectory = self.store.load_trajectory(&self.parent_run_id).await?;
        let assistant = trajectory
            .last()
            .context("forked delegate parent trajectory is empty")?;
        ensure!(
            assistant.message.role == Role::Assistant
                && assistant.message.content.iter().any(|content| {
                    matches!(
                        content,
                        MessageContent::ToolCall { id, name, .. }
                            if id == call_id && name == "delegate"
                    )
                }),
            "forked delegate call `{call_id}` is not in the latest parent assistant message"
        );
        ensure!(
            assistant.seq > 1,
            "forked delegate has no parent input message to inherit"
        );
        Ok(assistant.seq - 1)
    }
}
