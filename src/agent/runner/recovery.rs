use std::sync::Arc;

use anyhow::{Context, Result, ensure};

use crate::{
    artifact::ResultMetadata,
    model::{Message, MessageContent, Role},
    storage::{RunDirStore, RunState},
    trajectory::TrajectoryMessage,
};

use super::{AgentRunner, RunRequest, RunResult, lifecycle::RunMode};
use crate::agent::{
    compaction::estimate_message_tokens,
    task::{BackgroundTaskRecord, TaskManager},
};

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
        let request = RunRequest::from_stored(
            record.prompt,
            record.parent_run_id,
            record.depth,
            record.additional_instructions,
            &record.profile,
        )?;
        let plan = self.plan(&request);
        ensure!(
            record.model == plan.model,
            "run `{run_id}` used model `{}` but current profile selects `{}`",
            record.model,
            plan.model
        );
        self.run_with_mode(request, run_id, RunMode::Resume, lease.clone())
            .await
    }
}

pub(super) async fn append_background_results(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
    records: &[BackgroundTaskRecord],
) -> Result<u64> {
    let mut estimated_tokens = 0_u64;
    for record in records {
        let status = record.status().to_owned();
        let content = if record.kind == "tool" && record.state.is_terminal() {
            format!(
                "original_tool_call_id: {}\n{}",
                record
                    .origin_call_id
                    .as_deref()
                    .context("terminal tool task is missing its original tool-call id")?,
                record.model_content()
            )
        } else {
            record.model_content()
        };
        let metadata = record.result_metadata();
        let message = Message {
            role: Role::User,
            content: vec![MessageContent::BackgroundTaskResult {
                task_id: record.id.clone(),
                name: record.name.clone(),
                status,
                content,
                metadata,
            }],
        };
        let trajectory_record = store.append_message(run_id, &message).await?;
        estimated_tokens = estimated_tokens.saturating_add(estimate_message_tokens(&message));
        trajectory.push(trajectory_record);
    }
    Ok(estimated_tokens)
}

pub(super) async fn append_interrupted_tool_results(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
    tasks: &TaskManager,
) -> Result<()> {
    let Some(assistant_index) = trajectory
        .iter()
        .rposition(|record| record.compaction.is_none() && record.message.role == Role::Assistant)
    else {
        return Ok(());
    };
    let calls = trajectory[assistant_index].message.tool_calls();
    let assistant_created_at = trajectory[assistant_index].created_at;
    if calls.is_empty() {
        return Ok(());
    }
    let completed = trajectory[assistant_index + 1..]
        .iter()
        .flat_map(|record| &record.message.content)
        .filter_map(|content| match content {
            MessageContent::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    let missing = calls
        .into_iter()
        .filter(|call| !completed.contains(call.id.as_str()))
        .collect::<Vec<_>>();
    for call in missing {
        let task = tasks
            .find_undelivered_promotion(&call.id, &call.name, assistant_created_at)
            .await;
        let (content, is_error) = if let Some(task) = task {
            (
                serde_json::to_string(&serde_json::json!({
                    "task_id": task.id,
                    "status": task.status(),
                }))?,
                false,
            )
        } else {
            (
                format!(
                    "tool `{}` was interrupted before a durable result was recorded; its side effects are unknown. Inspect task state or the workspace before deciding whether to retry.",
                    call.name
                ),
                true,
            )
        };
        let message = Message {
            role: Role::Tool,
            content: vec![MessageContent::ToolResult {
                call_id: call.id,
                content,
                is_error,
                metadata: ResultMetadata::empty(),
            }],
        };
        trajectory.push(store.append_message(run_id, &message).await?);
    }
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
