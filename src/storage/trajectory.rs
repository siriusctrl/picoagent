use anyhow::{Result, bail, ensure};
use chrono::{DateTime, Utc};

use crate::{
    model::Message,
    trajectory::{CompactionMessage, TrajectoryMessage, message_ref},
};

use super::{MESSAGE_FORMAT, RunDirStore, ensure_run_exists, message_log};

impl RunDirStore {
    pub async fn append_message(
        &self,
        run_id: &str,
        message: &Message,
    ) -> Result<TrajectoryMessage> {
        self.append_classified_message(run_id, message, None, None)
            .await
    }

    pub(crate) async fn append_pending_input_message(
        &self,
        run_id: &str,
        message: &Message,
        pending_input_id: String,
    ) -> Result<TrajectoryMessage> {
        self.append_classified_message(run_id, message, Some(pending_input_id), None)
            .await
    }

    async fn append_classified_message(
        &self,
        run_id: &str,
        message: &Message,
        pending_input_id: Option<String>,
        compaction: Option<CompactionMessage>,
    ) -> Result<TrajectoryMessage> {
        self.append_classified_message_at(run_id, message, pending_input_id, compaction, Utc::now())
            .await
    }

    async fn append_classified_message_at(
        &self,
        run_id: &str,
        message: &Message,
        pending_input_id: Option<String>,
        compaction: Option<CompactionMessage>,
        created_at: DateTime<Utc>,
    ) -> Result<TrajectoryMessage> {
        let mut sequences = self.write_lock.lock().await;
        // Invalidate the fast path before any cancellable I/O. If this future is
        // dropped during either half of the commit, the next append must inspect
        // and repair the files instead of trusting a stale sequence cursor.
        let cached = sequences.remove(run_id);
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let _log_lock = message_log::exclusive_lock(&paths.directory).await?;
        let lengths = message_log::lengths(&paths.messages, &paths.message_metadata).await?;
        let next = match cached {
            Some(cursor)
                if cursor.messages_len == lengths.messages
                    && cursor.metadata_len == lengths.metadata =>
            {
                cursor.next_seq
            }
            _ => {
                let run = self.load_run(run_id).await?;
                ensure!(
                    run.message_format == MESSAGE_FORMAT,
                    "run {run_id} uses unsupported message format {}",
                    run.message_format
                );
                message_log::prepare_append(&paths.messages, &paths.message_metadata)
                    .await?
                    .saturating_add(1)
            }
        };
        let record = TrajectoryMessage {
            message_ref: message_ref(next),
            seq: next,
            created_at,
            message: message.clone(),
            pending_input_id,
            compaction,
        };
        message_log::append(&paths.messages, &paths.message_metadata, &record).await?;
        let lengths = message_log::lengths(&paths.messages, &paths.message_metadata).await?;
        sequences.insert(
            run_id.to_owned(),
            super::MessageCursor {
                next_seq: next.saturating_add(1),
                messages_len: lengths.messages,
                metadata_len: lengths.metadata,
            },
        );
        Ok(record)
    }

    /// Materialize one immutable parent record into a forked child run. The
    /// target must start empty and receive the source prefix in sequence.
    pub(crate) async fn append_forked_message(
        &self,
        run_id: &str,
        source: &TrajectoryMessage,
    ) -> Result<TrajectoryMessage> {
        let record = self
            .append_classified_message_at(
                run_id,
                &source.message,
                // Pending-input ids are run-local idempotency keys, not model
                // context. Copying one into the child could suppress an
                // unrelated child steering input with the same id.
                None,
                source.compaction.clone(),
                source.created_at,
            )
            .await?;
        ensure!(
            record.seq == source.seq && record.message_ref == source.message_ref,
            "forked trajectory record `{}` is not the next child message",
            source.message_ref
        );
        Ok(record)
    }

    pub(crate) async fn append_compaction_message(
        &self,
        run_id: &str,
        message: &Message,
        compaction: CompactionMessage,
    ) -> Result<TrajectoryMessage> {
        self.append_classified_message(run_id, message, None, Some(compaction))
            .await
    }

    pub async fn load_messages(&self, run_id: &str) -> Result<Vec<Message>> {
        Ok(self
            .load_trajectory(run_id)
            .await?
            .into_iter()
            .map(|record| record.message)
            .collect())
    }

    pub async fn load_trajectory(&self, run_id: &str) -> Result<Vec<TrajectoryMessage>> {
        let mut sequences = self.write_lock.lock().await;
        // A read may discover an orphan or corruption without repairing it.
        // Never let a later append fast-path around that validation result.
        sequences.remove(run_id);
        let paths = self.paths(run_id);
        let run = self.load_run(run_id).await?;
        ensure!(
            run.message_format == MESSAGE_FORMAT,
            "run {run_id} uses unsupported message format {}",
            run.message_format
        );
        let _log_lock = message_log::exclusive_lock(&paths.directory).await?;
        message_log::load(&paths.messages, &paths.message_metadata).await
    }

    /// Loads only the completed prefix hidden by the latest compaction.
    /// Messages still present in the active model context are never returned.
    pub async fn load_compacted_history(&self, run_id: &str) -> Result<Vec<TrajectoryMessage>> {
        let messages = self.load_trajectory(run_id).await?;
        let Some((state_message_ref, state)) = messages.iter().rev().find_map(|record| {
            record
                .compaction_state()
                .map(|state| (record.message_ref.as_str(), state))
        }) else {
            return Ok(Vec::new());
        };
        let Some(first_kept) = messages
            .iter()
            .position(|message| message.message_ref == state.first_kept_message_ref)
        else {
            bail!(
                "compacted state `{}` references missing first-kept message `{}`",
                state_message_ref,
                state.first_kept_message_ref
            );
        };
        Ok(messages
            .into_iter()
            .skip(1)
            .take(first_kept.saturating_sub(1))
            .filter(|message| message.compaction.is_none())
            .collect())
    }
}
