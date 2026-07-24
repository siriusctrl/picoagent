use std::{collections::VecDeque, sync::Arc};

use anyhow::Result;
use tokio::sync::Mutex;

use crate::{model::Message, storage::RunDirStore, trajectory::TrajectoryMessage};

#[derive(Clone, Default)]
pub(crate) struct AgentMailbox {
    state: Arc<Mutex<MailboxState>>,
}

#[derive(Default)]
struct MailboxState {
    accepting: bool,
    messages: VecDeque<Message>,
}

impl AgentMailbox {
    pub(super) async fn open(&self) {
        self.state.lock().await.accepting = true;
    }

    pub(super) async fn send(&self, message: Message) -> bool {
        let mut state = self.state.lock().await;
        if !state.accepting {
            return false;
        }
        state.messages.push_back(message);
        true
    }

    pub(super) async fn queue(&self, message: Message) {
        self.state.lock().await.messages.push_back(message);
    }

    pub(super) async fn seal(&self) {
        self.state.lock().await.accepting = false;
    }

    pub(super) async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.accepting = false;
        state.messages.clear();
    }

    #[cfg(test)]
    pub(super) async fn is_empty(&self) -> bool {
        self.state.lock().await.messages.is_empty()
    }

    pub(crate) async fn append_messages(
        &self,
        store: &RunDirStore,
        run_id: &str,
        trajectory: &mut Vec<TrajectoryMessage>,
    ) -> Result<Vec<TrajectoryMessage>> {
        self.append_messages_at_boundary(store, run_id, trajectory, false)
            .await
    }

    /// Seal an empty mailbox at the activity's final-response boundary.
    ///
    /// A concurrent sender that wins the mailbox lock is included in this
    /// activity. A sender that arrives after the empty mailbox is sealed queues
    /// a new activity through the owning handle record instead.
    pub(crate) async fn finish_boundary(
        &self,
        store: &RunDirStore,
        run_id: &str,
        trajectory: &mut Vec<TrajectoryMessage>,
    ) -> Result<Vec<TrajectoryMessage>> {
        self.append_messages_at_boundary(store, run_id, trajectory, true)
            .await
    }

    async fn append_messages_at_boundary(
        &self,
        store: &RunDirStore,
        run_id: &str,
        trajectory: &mut Vec<TrajectoryMessage>,
        seal_if_empty: bool,
    ) -> Result<Vec<TrajectoryMessage>> {
        let mut state = self.state.lock().await;
        if state.messages.is_empty() {
            if seal_if_empty {
                state.accepting = false;
            }
            return Ok(Vec::new());
        }
        // Mailbox input is process-local and at-most-once. Remove it before
        // cancellable persistence so stop or process failure can lose an
        // uncommitted input, but can never replay it into a later activity.
        let messages = state.messages.drain(..).collect::<Vec<_>>();
        drop(state);
        let records = store.append_messages(run_id, &messages).await?;
        trajectory.extend(records.iter().cloned());
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Message, Role};

    use super::AgentMailbox;

    #[tokio::test]
    async fn failed_append_does_not_requeue_process_local_input() {
        let workspace = tempfile::TempDir::new().unwrap();
        let store = crate::storage::RunDirStore::new(workspace.path());
        let mailbox = AgentMailbox::default();
        mailbox
            .queue(Message::text(Role::User, "at most once"))
            .await;

        let error = mailbox
            .append_messages(&store, "missing-run", &mut Vec::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing-run"));
        assert!(mailbox.is_empty().await);
    }
}
