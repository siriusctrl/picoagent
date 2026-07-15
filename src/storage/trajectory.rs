use std::path::Path;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{fs::OpenOptions, io::AsyncWriteExt};

use crate::{
    model::Message,
    trajectory::{CompactedHistory, CompactedHistorySource, TrajectoryMessage},
};

use super::{RunDirStore, append_json_line, ensure_run_exists};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMessage {
    #[serde(default, rename = "version")]
    _version: u32,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    seq: Option<u64>,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    message: Message,
}

impl From<&TrajectoryMessage> for StoredMessage {
    fn from(record: &TrajectoryMessage) -> Self {
        Self {
            _version: 1,
            message_id: Some(record.message_ref.clone()),
            seq: Some(record.seq),
            created_at: Some(record.created_at),
            message: record.message.clone(),
        }
    }
}

impl StoredMessage {
    fn into_trajectory(self, fallback_seq: u64) -> TrajectoryMessage {
        let seq = self.seq.unwrap_or(fallback_seq);
        TrajectoryMessage {
            message_ref: self
                .message_id
                .unwrap_or_else(|| format!("legacy_{fallback_seq:08}")),
            seq,
            created_at: self.created_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            message: self.message,
        }
    }
}

impl RunDirStore {
    pub async fn append_message(
        &self,
        run_id: &str,
        message: &Message,
    ) -> Result<TrajectoryMessage> {
        let mut sequences = self.write_lock.lock().await;
        let paths = self.paths(run_id);
        ensure_run_exists(&paths).await?;
        let next = match sequences.get(run_id).copied() {
            Some(next) => next,
            None => prepare_jsonl_append::<StoredMessage>(&paths.messages)
                .await?
                .saturating_add(1),
        };
        let record = TrajectoryMessage {
            message_ref: format!("msg_{}", ulid::Ulid::new()),
            seq: next,
            created_at: Utc::now(),
            message: message.clone(),
        };
        append_json_line(&paths.messages, &StoredMessage::from(&record)).await?;
        sequences.insert(run_id.to_owned(), next.saturating_add(1));
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
        let path = self.paths(run_id).messages;
        Ok(
            read_jsonl::<StoredMessage>(&path, "stored trajectory message")
                .await?
                .into_iter()
                .enumerate()
                .map(|(index, stored)| stored.into_trajectory(index as u64 + 1))
                .collect(),
        )
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
}

#[async_trait]
impl CompactedHistorySource for RunDirStore {
    async fn load_compacted_history(&self, run_id: &str) -> Result<CompactedHistory> {
        let Some(checkpoint) = self.load_latest_compaction(run_id).await? else {
            return Ok(CompactedHistory::default());
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
        let compacted = messages
            .into_iter()
            .skip(1)
            .take(first_kept.saturating_sub(1))
            .collect();
        Ok(CompactedHistory {
            messages: compacted,
        })
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
