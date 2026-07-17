use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};

use crate::{model::Message, trajectory::TrajectoryMessage};

use super::{MESSAGE_FORMAT, RunDirStore, append_json_line, ensure_run_exists, message_log};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub version: u32,
    pub checkpoint_id: String,
    pub created_at: DateTime<Utc>,
    pub strategy: String,
    pub previous_checkpoint_id: Option<String>,
    pub covered_through_message_ref: String,
    pub first_kept_message_ref: String,
    pub summary: String,
    pub provider: String,
    pub model: String,
    pub tokens_before: u64,
    pub summary_input_tokens: Option<u64>,
    pub summary_output_tokens: Option<u64>,
    pub compacted_message_count: usize,
}

impl RunDirStore {
    pub async fn append_message(
        &self,
        run_id: &str,
        message: &Message,
    ) -> Result<TrajectoryMessage> {
        self.append_message_with_ref(run_id, message, format!("msg_{}", ulid::Ulid::new()))
            .await
    }

    pub(crate) async fn append_message_with_ref(
        &self,
        run_id: &str,
        message: &Message,
        message_ref: String,
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
            message_ref,
            seq: next,
            created_at: Utc::now(),
            message: message.clone(),
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

    pub async fn append_compaction(
        &self,
        run_id: &str,
        checkpoint: &CompactionCheckpoint,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        prepare_jsonl_append::<CompactionCheckpoint>(&paths.compactions).await?;
        append_json_line(&paths.compactions, checkpoint).await
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

    pub async fn load_compactions(&self, run_id: &str) -> Result<Vec<CompactionCheckpoint>> {
        let path = self.paths(run_id).compactions;
        read_jsonl(&path, "stored compaction checkpoint").await
    }

    pub async fn load_latest_compaction(
        &self,
        run_id: &str,
    ) -> Result<Option<CompactionCheckpoint>> {
        Ok(self.load_compactions(run_id).await?.pop())
    }

    /// Loads only the completed prefix hidden by the latest compaction.
    /// Messages still present in the active model context are never returned.
    pub async fn load_compacted_history(&self, run_id: &str) -> Result<Vec<TrajectoryMessage>> {
        let Some(checkpoint) = self.load_latest_compaction(run_id).await? else {
            return Ok(Vec::new());
        };
        let messages = self.load_trajectory(run_id).await?;
        let Some(first_kept) = messages
            .iter()
            .position(|message| message.message_ref == checkpoint.first_kept_message_ref)
        else {
            bail!(
                "compaction checkpoint `{}` references missing first-kept message `{}`",
                checkpoint.checkpoint_id,
                checkpoint.first_kept_message_ref
            );
        };
        Ok(messages
            .into_iter()
            .skip(1)
            .take(first_kept.saturating_sub(1))
            .collect())
    }
}

async fn read_jsonl<T: DeserializeOwned>(path: &Path, record_name: &str) -> Result<Vec<T>> {
    let bytes = read_optional_bytes(path).await?;
    let has_torn_tail = !bytes.is_empty() && !bytes.ends_with(b"\n");
    let mut records = Vec::new();
    let mut lines = bytes.split(|byte| *byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(record) => records.push(record),
            Err(_) if has_torn_tail && lines.peek().is_none() => break,
            Err(error) => {
                return Err(error).with_context(|| format!("parse {record_name}"));
            }
        }
    }
    Ok(records)
}

async fn prepare_jsonl_append<T: DeserializeOwned>(path: &Path) -> Result<u64> {
    let bytes = read_optional_bytes(path).await?;
    if bytes.is_empty() {
        return Ok(0);
    }

    let complete_end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let complete = &bytes[..complete_end];
    let complete_records = parse_complete_jsonl::<T>(complete)?;
    if complete_end == bytes.len() {
        return Ok(complete_records);
    }

    let tail = &bytes[complete_end..];
    let valid_tail =
        !tail.iter().all(u8::is_ascii_whitespace) && serde_json::from_slice::<T>(tail).is_ok();
    let mut file = OpenOptions::new()
        .write(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {} for tail recovery", path.display()))?;
    if valid_tail {
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok(complete_records.saturating_add(1))
    } else {
        file.set_len(complete_end as u64).await?;
        file.flush().await?;
        Ok(complete_records)
    }
}

fn parse_complete_jsonl<T: DeserializeOwned>(bytes: &[u8]) -> Result<u64> {
    let mut count = 0_u64;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        serde_json::from_slice::<T>(line).context("parse complete JSONL record")?;
        count = count.saturating_add(1);
    }
    Ok(count)
}

async fn read_optional_bytes(path: &Path) -> Result<Vec<u8>> {
    match tokio::fs::read(path).await {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}
