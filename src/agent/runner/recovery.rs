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
        let content = record.model_content();
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

pub(super) fn remaining_preview_budget(limit: usize, trajectory: &[TrajectoryMessage]) -> usize {
    let used = trajectory
        .iter()
        .flat_map(|record| &record.message.content)
        .map(|content| match content {
            MessageContent::ToolResult { metadata, .. }
            | MessageContent::BackgroundTaskResult { metadata, .. } => metadata.preview_bytes,
            _ => 0,
        })
        .fold(0_usize, usize::saturating_add);
    limit.saturating_sub(used)
}

pub(super) async fn append_interrupted_tool_results(
    store: &RunDirStore,
    run_id: &str,
    trajectory: &mut Vec<TrajectoryMessage>,
) -> Result<usize> {
    let Some(assistant_index) = trajectory
        .iter()
        .rposition(|record| record.compaction.is_none() && record.message.role == Role::Assistant)
    else {
        return Ok(0);
    };
    let calls = trajectory[assistant_index].message.tool_calls();
    if calls.is_empty() {
        return Ok(0);
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
    let mut charged_preview_bytes = 0_usize;
    for call in missing {
        let content = format!(
            "tool `{}` was interrupted before a durable result was recorded; its side effects are unknown. Inspect task state or the workspace before deciding whether to retry.",
            call.name
        );
        let preview_bytes = content.len();
        charged_preview_bytes = charged_preview_bytes.saturating_add(preview_bytes);
        let message = Message {
            role: Role::Tool,
            content: vec![MessageContent::ToolResult {
                call_id: call.id,
                content,
                is_error: true,
                metadata: ResultMetadata {
                    artifact: None,
                    preview_bytes,
                },
            }],
        };
        trajectory.push(store.append_message(run_id, &message).await?);
    }
    Ok(charged_preview_bytes)
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

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn preview_budget_restoration_uses_persisted_preview_bytes() {
        let trajectory = vec![TrajectoryMessage {
            message_ref: "msg-1".to_owned(),
            seq: 1,
            created_at: Utc::now(),
            message: Message {
                role: Role::Tool,
                content: vec![MessageContent::ToolResult {
                    call_id: "call-1".to_owned(),
                    content: "a long artifact envelope that is not itself preview bytes".to_owned(),
                    is_error: false,
                    metadata: ResultMetadata {
                        artifact: None,
                        preview_bytes: 7,
                    },
                }],
            },
            compaction: None,
        }];

        assert_eq!(remaining_preview_budget(100, &trajectory), 93);
    }
}
