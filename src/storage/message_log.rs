use std::{io::SeekFrom, path::Path};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::OpenOptions,
    io::{AsyncSeekExt, AsyncWriteExt},
};

use crate::{
    model::Message,
    trajectory::{CompactionMessage, TrajectoryMessage, message_ref, message_ref_seq},
};

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
        rename = "_pico",
        default,
        skip_serializing_if = "LocalState::is_empty"
    )]
    local: LocalState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionMessage>,
}

impl LocalState {
    fn is_empty(&self) -> bool {
        self.pending_input_id.is_none() && self.compaction.is_none()
    }
}

struct JsonlFile {
    records: Vec<TrajectoryMessage>,
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

pub(super) async fn append(path: &Path, record: &TrajectoryMessage) -> Result<()> {
    record
        .message
        .validate()
        .context("validate message before persistence")?;
    let stored = StoredMessage {
        message_ref: record.message_ref.clone(),
        created_at: record.created_at,
        message: record.message.clone(),
        local: LocalState {
            pending_input_id: record.pending_input_id.clone(),
            compaction: record.compaction.clone(),
        },
    };
    let mut line = serde_json::to_vec(&stored).context("serialize stored message")?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {} for append", path.display()))?;
    file.write_all(&line)
        .await
        .with_context(|| format!("append {}", path.display()))?;
    file.flush().await?;
    file.sync_data()
        .await
        .with_context(|| format!("sync {}", path.display()))
}

/// Load the committed prefix. A newline is the commit marker, so a viewer can
/// safely ignore a concurrently written or crash-torn final line without a
/// lock.
pub(super) async fn load(path: &Path) -> Result<Vec<TrajectoryMessage>> {
    Ok(read_jsonl(path).await?.records)
}

/// Validate the committed prefix and remove an uncommitted tail before the
/// sole writer resumes appending.
pub(super) async fn prepare_append(path: &Path) -> Result<u64> {
    let log = read_jsonl(path).await?;
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
    Ok(log.records.len() as u64)
}

async fn read_jsonl(path: &Path) -> Result<JsonlFile> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read initialized message log {}", path.display()))?;
    let original_len = bytes.len() as u64;
    let mut records = Vec::new();
    let mut committed_end = 0_usize;

    for line_with_newline in bytes.split_inclusive(|byte| *byte == b'\n') {
        if !line_with_newline.ends_with(b"\n") {
            break;
        }
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
        let seq = message_ref_seq(&stored.message_ref)
            .with_context(|| format!("stored message has invalid ref `{}`", stored.message_ref))?;
        let expected_seq = records.len() as u64 + 1;
        ensure!(
            seq == expected_seq && stored.message_ref == message_ref(expected_seq),
            "message ref `{}` is not the expected `m{expected_seq}`",
            stored.message_ref
        );
        records.push(TrajectoryMessage {
            message_ref: stored.message_ref,
            seq,
            created_at: stored.created_at,
            message: stored.message,
            pending_input_id: stored.local.pending_input_id,
            compaction: stored.local.compaction,
        });
        committed_end += line_with_newline.len();
    }

    Ok(JsonlFile {
        records,
        original_len,
        committed_end: committed_end as u64,
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
