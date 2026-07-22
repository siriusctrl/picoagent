use std::sync::Arc;

use anyhow::{Result, ensure};

use crate::{
    model::{Message, MessageContent, Role},
    storage::{RunDirStore, RunState},
    trajectory::TrajectoryMessage,
};

use super::{AgentRunner, RunRequest, RunResult, lifecycle::RunMode};
use crate::agent::{compaction::estimate_message_tokens, task::TaskOutputNotice};

const RESTART_REMINDER: &str = "<runtime-reminder>\nThe previous fiasco process stopped after the last complete checkpoint. Any uncommitted model/tool turn was discarded, but its workspace or external side effects may already have occurred. Inspect the current state before retrying any operation.\n</runtime-reminder>";

impl AgentRunner {
    pub async fn resume(self: &Arc<Self>, run_id: impl Into<String>) -> Result<RunResult> {
        self.resume_with_parent(run_id.into(), None, None).await
    }

    pub(crate) async fn resume_child(
        self: &Arc<Self>,
        run_id: String,
        expected_parent_run_id: &str,
        cleanup_done: tokio::sync::oneshot::Sender<()>,
    ) -> Result<RunResult> {
        self.resume_with_parent(run_id, Some(expected_parent_run_id), Some(cleanup_done))
            .await
    }

    async fn resume_with_parent(
        self: &Arc<Self>,
        run_id: String,
        expected_parent_run_id: Option<&str>,
        cleanup_done: Option<tokio::sync::oneshot::Sender<()>>,
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
        ensure!(
            !matches!(
                record.state,
                RunState::Completed | RunState::Cancelled | RunState::Closed
            ),
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
        let mode = match (record.state, expected_parent_run_id) {
            (RunState::Queued, _) => RunMode::New,
            (_, Some(_)) => RunMode::Continue,
            (_, None) => RunMode::Restart,
        };
        self.run_with_mode(request, run_id, mode, lease.clone(), cleanup_done)
            .await
    }
}

pub(super) async fn append_background_results(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
    notices: &[TaskOutputNotice],
) -> Result<u64> {
    if notices.is_empty() {
        return Ok(0);
    }
    let content = notices
        .iter()
        .map(|notice| MessageContent::BackgroundTask {
            task_id: notice.task_id.clone(),
            name: notice.name.clone(),
            output_seq: Some(notice.output.seq),
            status: Some(notice.output.status.as_str().to_owned()),
            content: notice.output.model_content(),
            metadata: notice.output.result_metadata(),
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
