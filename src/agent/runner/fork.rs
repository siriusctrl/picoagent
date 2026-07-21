use anyhow::{Result, ensure};

use crate::{
    model::{MessageContent, Role},
    trajectory::TrajectoryMessage,
};

use super::AgentRunner;

impl AgentRunner {
    pub(super) async fn materialize_fork_prefix(
        &self,
        parent_run_id: &str,
        child_run_id: &str,
        boundary: u64,
        child_prompt: &str,
        child: &mut Vec<TrajectoryMessage>,
    ) -> Result<()> {
        ensure!(
            parent_run_id != child_run_id,
            "forked run cannot inherit its own trajectory"
        );
        ensure!(boundary > 0, "fork boundary must be positive");
        if child.len() < boundary as usize {
            let parent = self.store.load_trajectory(parent_run_id).await?;
            ensure!(
                boundary <= parent.len() as u64,
                "fork boundary m{boundary} is not present in parent run `{parent_run_id}`"
            );
            let expected = &parent[..boundary as usize];
            for (index, (stored, source)) in child.iter().zip(expected).enumerate() {
                ensure!(
                    fork_records_equal(stored, source)?,
                    "forked child `{child_run_id}` message m{} differs from its frozen parent snapshot",
                    index + 1
                );
            }
            for source in &expected[child.len()..] {
                child.push(
                    self.store
                        .append_forked_message(child_run_id, source)
                        .await?,
                );
            }
        }
        if let Some(task_message) = child.get(boundary as usize) {
            ensure!(
                task_message.compaction.is_none()
                    && task_message.pending_input_id.is_none()
                    && task_message.message.role == Role::User
                    && task_message.message.content.iter().any(|content| {
                        matches!(content, MessageContent::Text { text } if text == child_prompt)
                    })
                    && task_message.message.content.iter().any(|content| {
                        matches!(content, MessageContent::RuntimeReminder { .. })
                    }),
                "forked child `{child_run_id}` does not begin its suffix with the delegated task"
            );
        }
        Ok(())
    }
}

fn fork_records_equal(left: &TrajectoryMessage, right: &TrajectoryMessage) -> Result<bool> {
    Ok(left.message_ref == right.message_ref
        && left.seq == right.seq
        && left.created_at == right.created_at
        && left.pending_input_id.is_none()
        && left.compaction == right.compaction
        && serde_json::to_value(&left.message)? == serde_json::to_value(&right.message)?)
}
