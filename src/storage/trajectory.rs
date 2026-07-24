use anyhow::{Result, bail, ensure};
use chrono::{DateTime, Utc};

use crate::{
    model::Message,
    trajectory::{CompactionMessage, TrajectoryMessage, message_ref},
};

use super::{MESSAGE_FORMAT, RunDirStore, ensure_run_exists, message_log};

impl RunDirStore {
    /// Append one ordered batch while preserving one JSON line and one `m<N>`
    /// ref per message. Complete lines become visible independently.
    pub async fn append_messages(
        &self,
        run_id: &str,
        messages: &[Message],
    ) -> Result<Vec<TrajectoryMessage>> {
        let created_at = Utc::now();
        self.append_classified_messages(
            run_id,
            messages
                .iter()
                .cloned()
                .map(|message| ClassifiedMessage {
                    message,
                    compaction: None,
                    created_at,
                })
                .collect(),
        )
        .await
    }

    pub async fn append_message(
        &self,
        run_id: &str,
        message: &Message,
    ) -> Result<TrajectoryMessage> {
        let mut records = self
            .append_messages(run_id, std::slice::from_ref(message))
            .await?;
        Ok(records.pop().expect("singleton append returned no message"))
    }

    async fn append_classified_messages(
        &self,
        run_id: &str,
        messages: Vec<ClassifiedMessage>,
    ) -> Result<Vec<TrajectoryMessage>> {
        ensure!(!messages.is_empty(), "message append must not be empty");
        let mut sequences = self.write_lock.lock().await;
        // Invalidate the fast path before cancellable I/O. A dropped append may
        // leave a non-newline-terminated tail that the next append must trim.
        let cached = sequences.remove(run_id);
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let next = match cached {
            Some(cursor) => cursor.next_seq,
            _ => {
                let run = self.load_run(run_id).await?;
                ensure!(
                    run.message_format == MESSAGE_FORMAT,
                    "run {run_id} uses unsupported message format {}",
                    run.message_format
                );
                message_log::prepare_append(&paths.messages)
                    .await?
                    .saturating_add(1)
            }
        };
        let records = messages
            .into_iter()
            .enumerate()
            .map(|(index, message)| {
                let seq = next.saturating_add(index as u64);
                TrajectoryMessage {
                    message_ref: message_ref(seq),
                    seq,
                    created_at: message.created_at,
                    message: message.message,
                    compaction: message.compaction,
                }
            })
            .collect::<Vec<_>>();
        message_log::append_messages(&paths.messages, &records).await?;
        let next_seq = next.saturating_add(records.len() as u64);
        sequences.insert(run_id.to_owned(), super::MessageCursor { next_seq });
        Ok(records)
    }

    pub(crate) async fn append_compaction_messages(
        &self,
        run_id: &str,
        request: &Message,
        state_message: &Message,
        state: crate::trajectory::CompactionState,
    ) -> Result<[TrajectoryMessage; 2]> {
        let created_at = Utc::now();
        let records = self
            .append_classified_messages(
                run_id,
                vec![
                    ClassifiedMessage {
                        message: request.clone(),
                        compaction: Some(CompactionMessage::Request),
                        created_at,
                    },
                    ClassifiedMessage {
                        message: state_message.clone(),
                        compaction: Some(CompactionMessage::State { state }),
                        created_at,
                    },
                ],
            )
            .await?;
        records
            .try_into()
            .map_err(|_| anyhow::anyhow!("compaction append did not contain two messages"))
    }

    /// Repair a crash-torn record or incomplete trailing tool turn before an
    /// existing run is loaded for a new activity.
    pub(crate) async fn prepare_resume(&self, run_id: &str) -> Result<()> {
        let mut sequences = self.write_lock.lock().await;
        sequences.remove(run_id);
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let record_count = message_log::prepare_append(&paths.messages).await?;
        sequences.insert(
            run_id.to_owned(),
            super::MessageCursor {
                next_seq: record_count.saturating_add(1),
            },
        );
        Ok(())
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
        let paths = self.paths(run_id);
        let run = self.load_run(run_id).await?;
        ensure!(
            run.message_format == MESSAGE_FORMAT,
            "run {run_id} uses unsupported message format {}",
            run.message_format
        );
        message_log::load(&paths.messages).await
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

struct ClassifiedMessage {
    message: Message,
    compaction: Option<CompactionMessage>,
    created_at: DateTime<Utc>,
}
