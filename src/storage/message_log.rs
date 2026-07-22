use std::{io::SeekFrom, path::Path};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::OpenOptions,
    io::{AsyncSeekExt, AsyncWriteExt, BufReader},
};

use crate::{
    model::Message,
    trajectory::{CompactionMessage, TrajectoryMessage, message_ref, message_ref_seq},
};

pub(crate) mod decoder;

use decoder::{CommittedCheckpointReader, CommittedRecord, DecodeResult};

/// One durable transcript line. The provider-neutral content blocks are kept
/// directly in the record so readers do not need a second file to reconstruct
/// the message that the runner will replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMessage {
    #[serde(rename = "ref")]
    message_ref: String,
    created_at: DateTime<Utc>,
    #[serde(flatten)]
    message: Message,
    #[serde(
        rename = "_fiasco",
        default,
        skip_serializing_if = "LocalState::is_empty"
    )]
    local: LocalState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<MessageCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionMessage>,
}

impl LocalState {
    fn is_empty(&self) -> bool {
        self.checkpoint.is_none() && self.pending_input_id.is_none() && self.compaction.is_none()
    }
}

/// Identifies one logical checkpoint while preserving one JSON line per
/// message. A reader publishes none of the lines until it has observed the
/// complete, contiguous group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct MessageCheckpoint {
    first_message_ref: String,
    index: u64,
    count: u64,
}

struct JsonlFile {
    records: Vec<TrajectoryMessage>,
    record_count: u64,
    original_len: u64,
    committed_end: u64,
}

pub(super) async fn initialize(run_directory: &Path, messages_path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(messages_path)
        .await
        .with_context(|| format!("create message log file {}", messages_path.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("sync message log file {}", messages_path.display()))?;
    sync_directory(run_directory).await
}

pub(super) async fn append_checkpoint(path: &Path, records: &[TrajectoryMessage]) -> Result<()> {
    ensure!(!records.is_empty(), "message checkpoint must not be empty");
    let first_message_ref = records[0].message_ref.clone();
    let count = records.len() as u64;
    let mut bytes = Vec::new();
    for (index, record) in records.iter().enumerate() {
        record
            .message
            .validate()
            .context("validate message before persistence")?;
        let expected_seq = records[0].seq.saturating_add(index as u64);
        ensure!(
            record.seq == expected_seq && record.message_ref == message_ref(expected_seq),
            "checkpoint message ref `{}` is not the expected `{}`",
            record.message_ref,
            message_ref(expected_seq)
        );
        let stored = StoredMessage {
            message_ref: record.message_ref.clone(),
            created_at: record.created_at,
            message: record.message.clone(),
            local: LocalState {
                checkpoint: Some(MessageCheckpoint {
                    first_message_ref: first_message_ref.clone(),
                    index: index as u64,
                    count,
                }),
                pending_input_id: record.pending_input_id.clone(),
                compaction: record.compaction.clone(),
            },
        };
        serde_json::to_writer(&mut bytes, &stored).context("serialize stored message")?;
        bytes.push(b'\n');
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {} for append", path.display()))?;
    file.write_all(&bytes)
        .await
        .with_context(|| format!("append {}", path.display()))?;
    file.flush().await?;
    file.sync_data()
        .await
        .with_context(|| format!("sync {}", path.display()))
}

/// Load the committed prefix. A checkpoint is visible only after all of its
/// newline-terminated message lines are present, so viewers can ignore both a
/// crash-torn line and complete lines from an incomplete final checkpoint.
pub(super) async fn load(path: &Path) -> Result<Vec<TrajectoryMessage>> {
    Ok(read_jsonl(path, true).await?.records)
}

/// Validate the committed prefix and remove an uncommitted tail before the
/// sole writer resumes appending.
pub(super) async fn prepare_append(path: &Path) -> Result<u64> {
    let log = read_jsonl(path, false).await?;
    if log.original_len != log.committed_end {
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(path)
            .await
            .with_context(|| format!("open {} for trajectory recovery", path.display()))?;
        file.set_len(log.committed_end)
            .await
            .with_context(|| format!("truncate {} for trajectory recovery", path.display()))?;
        file.seek(SeekFrom::End(0)).await?;
        file.flush().await?;
        file.sync_data()
            .await
            .with_context(|| format!("sync recovered trajectory file {}", path.display()))?;
    }
    Ok(log.record_count)
}

async fn read_jsonl(path: &Path, collect_records: bool) -> Result<JsonlFile> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("read initialized message log {}", path.display()))?;
    let mut reader = CommittedCheckpointReader::new(BufReader::new(file), path.to_owned());
    let mut records = Vec::new();
    let mut record_count = 0_u64;
    let mut committed_end = 0;
    while let DecodeResult::Checkpoint(checkpoint) = reader.next_checkpoint().await? {
        committed_end = checkpoint.committed_end;
        record_count = record_count
            .checked_add(checkpoint.records.len() as u64)
            .context("committed message count overflow")?;
        for record in checkpoint.records {
            let CommittedRecord {
                trajectory,
                raw,
                source_offset,
            } = record;
            drop(raw);
            let _ = source_offset;
            if collect_records {
                records.push(trajectory);
            }
        }
    }
    debug_assert_eq!(committed_end, reader.committed_end());

    Ok(JsonlFile {
        records,
        record_count,
        original_len: reader.bytes_read(),
        committed_end,
    })
}

fn parse_stored_line(path: &Path, line_with_newline: &[u8]) -> Result<StoredMessage> {
    let line = &line_with_newline[..line_with_newline.len() - 1];
    ensure!(
        !line.iter().all(u8::is_ascii_whitespace),
        "blank line in {}",
        path.display()
    );
    let stored: StoredMessage = serde_json::from_slice(line)
        .with_context(|| format!("parse completed message in {}", path.display()))?;
    stored
        .message
        .validate()
        .with_context(|| format!("validate completed message in {}", path.display()))?;
    Ok(stored)
}

fn trajectory_record(stored: StoredMessage, expected_seq: u64) -> Result<TrajectoryMessage> {
    let seq = message_ref_seq(&stored.message_ref)
        .with_context(|| format!("stored message has invalid ref `{}`", stored.message_ref))?;
    ensure!(
        seq == expected_seq && stored.message_ref == message_ref(expected_seq),
        "message ref `{}` is not the expected `m{expected_seq}`",
        stored.message_ref
    );
    Ok(TrajectoryMessage {
        message_ref: stored.message_ref,
        seq,
        created_at: stored.created_at,
        message: stored.message,
        pending_input_id: stored.local.pending_input_id,
        compaction: stored.local.compaction,
    })
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let directory = std::fs::File::open(&path)
            .with_context(|| format!("open {} for directory sync", path.display()))?;
        directory
            .sync_all()
            .with_context(|| format!("sync directory {}", path.display()))
    })
    .await
    .context("join message log directory sync task")?
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}
