use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use ulid::Ulid;

use crate::{
    agent::runner::RunRequest,
    events::{RuntimeEvent, RuntimeEventKind},
    prompts::agent_prompts,
    tools::{RawToolOutput, ToolContext},
};

use super::super::{BackgroundTaskRecord, TaskManager};

impl TaskManager {
    pub async fn delegate(
        self: &Arc<Self>,
        name: String,
        prompt: String,
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
        let child_run_id = Ulid::new().to_string();
        let child_remaining_delegation_depth = self.remaining_delegation_depth.saturating_sub(1);
        let child_request = RunRequest::general_task(
            prompt.clone(),
            self.parent_run_id.clone(),
            self.parent_depth + 1,
            agent_prompts().general_task.clone(),
            child_remaining_delegation_depth,
        );
        self.runner
            .prepare_run(&child_request, &child_run_id)
            .await?;
        let task_id = self
            .create_agent_task(
                name.clone(),
                child_run_id,
                prompt,
                origin_call_id.to_owned(),
            )
            .await?;
        self.launch_agent_task(task_id.clone());
        self.get(&task_id).await
    }

    fn launch_agent_task(self: &Arc<Self>, task_id: String) {
        let manager = self.clone();
        let tracked_task_id = task_id.clone();
        let (start, wait_for_tracking) = tokio::sync::oneshot::channel();
        let (cleanup_done, wait_for_cleanup) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            if wait_for_tracking.await.is_err() {
                return;
            }
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
            let permit = match manager.subagent_slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(error) => {
                    manager
                        .finish_agent_failed_activity(
                            &task_id,
                            &task_name,
                            &child_run_id,
                            error.into(),
                        )
                        .await;
                    return;
                }
            };
            let Some(record) = (match manager.begin_agent_activity(&task_id).await {
                Ok(record) => record,
                Err(error) => {
                    manager
                        .finish_agent_failed_activity(&task_id, &task_name, &child_run_id, error)
                        .await;
                    return;
                }
            }) else {
                return;
            };
            manager
                .emit_agent_started(&task_id, &task_name, &child_run_id, &prompt)
                .await;
            let validation = async {
                let child = manager.store.load_run(&child_run_id).await?;
                manager.validate_child_run(&record, &child)
            }
            .await;
            if let Err(error) = validation {
                manager
                    .finish_agent_failed_activity(&task_id, &task_name, &child_run_id, error)
                    .await;
                return;
            }
            let outcome = manager.run_agent_child(&child_run_id, cleanup_done).await;
            drop(permit);
            manager
                .finish_agent_child(outcome, &task_id, &task_name, child_run_id)
                .await;
        });
        self.track_agent(tracked_task_id, handle, wait_for_cleanup);
        let _ = start.send(());
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
                RuntimeEventKind::SubagentActivityStarted {
                    child_run_id: child_run_id.to_owned(),
                    task: prompt.to_owned(),
                },
            ))
            .await;
    }

    async fn run_agent_child(
        &self,
        child_run_id: &str,
        cleanup_done: tokio::sync::oneshot::Sender<()>,
    ) -> Result<crate::agent::runner::RunResult> {
        self.runner
            .resume_child(child_run_id.to_owned(), &self.parent_run_id, cleanup_done)
            .await
    }

    async fn finish_agent_child(
        self: &Arc<Self>,
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
                    Err(error) => {
                        self.finish_agent_failed_activity(task_id, task_name, &child_run_id, error)
                            .await
                    }
                }
            }
            Err(error) => {
                self.finish_agent_failed_activity(task_id, task_name, &child_run_id, error)
                    .await;
            }
        }
    }

    async fn begin_agent_activity(&self, task_id: &str) -> Result<Option<BackgroundTaskRecord>> {
        let mut records = self.records.lock().await;
        let mut record = records
            .get(task_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown background task `{task_id}`"))?;
        ensure!(record.kind == "agent", "task `{task_id}` is not an agent");
        if !record.state.is_active() || record.paused {
            return Ok(None);
        }
        if record.state == super::super::BackgroundTaskState::Queued {
            record.state = super::super::BackgroundTaskState::Running;
            self.persist(&record).await?;
            records.insert(task_id.to_owned(), record.clone());
            drop(records);
            self.signal_activity();
        }
        Ok(Some(record))
    }

    pub(crate) async fn activate_agent_if_pending(self: &Arc<Self>, task_id: &str) -> Result<bool> {
        let mut records = self.records.lock().await;
        let record = records
            .get(task_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown background task `{task_id}`"))?;
        ensure!(record.kind == "agent", "task `{task_id}` is not an agent");
        if record.state != super::super::BackgroundTaskState::Idle || record.paused {
            return Ok(false);
        }
        let child_run_id = record
            .child_run_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("agent task is missing child_run_id"))?;
        let has_steer = self.store.has_pending_user_input(&child_run_id).await?;
        if record.pending_followups.is_empty() && !has_steer {
            return Ok(false);
        }
        for input in &record.pending_followups {
            self.store
                .enqueue_user_input_with_id(&child_run_id, input.id.clone(), input.message.clone())
                .await?;
        }
        let child = self.store.load_run(&child_run_id).await?;
        ensure!(
            child.state == crate::storage::RunState::Idle,
            "idle agent child `{child_run_id}` is unexpectedly {:?}",
            child.state
        );
        let mut running = record;
        running.pending_followups.clear();
        running.state = super::super::BackgroundTaskState::Running;
        self.persist(&running).await?;
        records.insert(task_id.to_owned(), running.clone());
        drop(records);
        self.signal_activity();
        self.launch_agent_task(task_id.to_owned());
        Ok(true)
    }
}
