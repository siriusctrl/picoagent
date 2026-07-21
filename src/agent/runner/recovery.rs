use std::sync::Arc;

use anyhow::{Context, Result, ensure};

use crate::{
    artifact::ResultMetadata,
    model::{Message, MessageContent, Role},
    storage::{RunDirStore, RunState},
    trajectory::TrajectoryMessage,
};

use super::{AgentRunner, RunRequest, RunResult, lifecycle::RunMode};
use crate::agent::{compaction::estimate_message_tokens, task::BackgroundTaskRecord};

const RESTART_REMINDER: &str = "<runtime-reminder>\nThe previous picoagent process stopped after the last complete checkpoint. Any uncommitted model/tool turn was discarded, but its workspace or external side effects may already have occurred. Inspect the current state before retrying any operation.\n</runtime-reminder>";

impl AgentRunner {
    pub async fn resume(self: &Arc<Self>, run_id: impl Into<String>) -> Result<RunResult> {
        self.resume_with_parent(run_id.into(), None).await
    }

    pub(crate) async fn resume_child(
        self: &Arc<Self>,
        run_id: String,
        expected_parent_run_id: &str,
    ) -> Result<RunResult> {
        self.resume_with_parent(run_id, Some(expected_parent_run_id))
            .await
    }

    async fn resume_with_parent(
        self: &Arc<Self>,
        run_id: String,
        expected_parent_run_id: Option<&str>,
    ) -> Result<RunResult> {
        let lease = self.store.acquire_run_lease(&run_id).await?;
        let record = self.store.load_run(&run_id).await?;
        match expected_parent_run_id {
            Some(parent_run_id) => ensure!(
                record.parent_run_id.as_deref() == Some(parent_run_id),
                "child run `{run_id}` does not belong to parent `{parent_run_id}`"
            ),
            None => ensure!(
                record.parent_run_id.is_none(),
                "run `{run_id}` is a child run; resume its parent `{}` instead",
                record.parent_run_id.as_deref().unwrap_or_default()
            ),
        }
        if expected_parent_run_id.is_some() && record.state == RunState::Completed {
            let final_output = tokio::fs::read_to_string(&self.store.paths(&run_id).final_output)
                .await
                .with_context(|| format!("read completed child run `{run_id}` final output"))?;
            return Ok(RunResult {
                run_id,
                final_output,
            });
        }
        ensure!(
            !matches!(record.state, RunState::Completed | RunState::Cancelled),
            "run `{run_id}` is already {:?}",
            record.state
        );
        ensure!(
            record.cwd == self.workspace,
            "run `{run_id}` belongs to workspace {}, not {}",
            record.cwd.display(),
            self.workspace.display()
        );
        ensure!(
            record.provider == self.provider.name(),
            "run `{run_id}` used provider `{}` but current provider is `{}`",
            record.provider,
            self.provider.name()
        );
        record.verify_provider_resume_fingerprint(&self.provider.resume_fingerprint())?;
        let request = RunRequest::from_stored(&record)?;
        let plan = self.plan(&request);
        ensure!(
            record.model == plan.model,
            "run `{run_id}` used model `{}` but current profile selects `{}`",
            record.model,
            plan.model
        );
        ensure!(
            record.model_modalities == plan.modalities,
            "run `{run_id}` used model modalities {:?} but current configuration selects {:?}",
            record.model_modalities,
            plan.modalities
        );
        let mode = if record.state == RunState::Queued {
            RunMode::New
        } else {
            RunMode::Resume
        };
        self.run_with_mode(request, run_id, mode, lease.clone())
            .await
    }
}

pub(super) async fn append_background_results(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
    records: &[BackgroundTaskRecord],
) -> Result<u64> {
    if records.is_empty() {
        return Ok(0);
    }
    let content = records
        .iter()
        .map(|record| {
            let (status, content, metadata) = if record.state.is_terminal() {
                let metadata = record.result_metadata();
                (
                    Some(record.status().to_owned()),
                    record.model_content(),
                    metadata,
                )
            } else {
                (
                    None,
                    "The task is still running in the background.".to_owned(),
                    ResultMetadata::empty(),
                )
            };
            MessageContent::BackgroundTask {
                task_id: record.id.clone(),
                name: record.name.clone(),
                status,
                content,
                metadata,
            }
        })
        .collect::<Vec<_>>();
    let message = Message {
        role: Role::User,
        content,
    };
    let estimated_tokens = estimate_message_tokens(&message);
    let trajectory_record = store.append_message(run_id, &message).await?;
    trajectory.push(trajectory_record);
    Ok(estimated_tokens)
}

pub(super) async fn append_restart_reminder(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
) -> Result<()> {
    let message = Message {
        role: Role::User,
        content: vec![MessageContent::RuntimeReminder {
            text: RESTART_REMINDER.to_owned(),
        }],
    };
    trajectory.push(store.append_message(run_id, &message).await?);
    Ok(())
}

pub(super) fn resumable_final_text(trajectory: &[TrajectoryMessage]) -> Option<String> {
    let record = trajectory.last()?;
    if record.compaction.is_some() {
        return None;
    }
    let message = &record.message;
    (message.role == Role::Assistant && message.tool_calls().is_empty())
        .then(|| message.visible_text())
}
