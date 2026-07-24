use std::{io::SeekFrom, path::Path};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::OpenOptions,
    io::{AsyncSeekExt, AsyncWriteExt, BufReader},
};

use crate::{
    model::{Message, MessageContent, Role},
    trajectory::{CompactionMessage, TrajectoryMessage, message_ref, message_ref_seq},
};

pub(crate) mod decoder;
mod transcript;

pub use transcript::TranscriptTimeline;

use decoder::CompleteLineReader;

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
    compaction: Option<CompactionMessage>,
}

impl LocalState {
    fn is_empty(&self) -> bool {
        self.compaction.is_none()
    }
}

struct JsonlFile {
    records: Vec<TrajectoryMessage>,
    record_count: u64,
    original_len: u64,
    append_end: u64,
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

pub(super) async fn append_messages(path: &Path, records: &[TrajectoryMessage]) -> Result<()> {
    ensure!(!records.is_empty(), "message append must not be empty");
    let mut bytes = Vec::new();
    for (index, record) in records.iter().enumerate() {
        record
            .message
            .validate()
            .context("validate message before persistence")?;
        let expected_seq = records[0].seq.saturating_add(index as u64);
        ensure!(
            record.seq == expected_seq && record.message_ref == message_ref(expected_seq),
            "appended message ref `{}` is not the expected `{}`",
            record.message_ref,
            message_ref(expected_seq)
        );
        let stored = StoredMessage {
            message_ref: record.message_ref.clone(),
            created_at: record.created_at,
            message: record.message.clone(),
            local: LocalState {
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

/// Load every complete newline-terminated message. A live viewer may observe a
/// prefix of the final tool turn while its batch is being written.
pub(super) async fn load(path: &Path) -> Result<Vec<TrajectoryMessage>> {
    Ok(read_jsonl(path, true).await?.records)
}

/// Repair the physical tail and discard a semantically incomplete final tool
/// turn before the sole writer resumes appending.
pub(super) async fn prepare_append(path: &Path) -> Result<u64> {
    let log = read_jsonl(path, false).await?;
    if log.original_len != log.append_end {
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(false)
            .open(path)
            .await
            .with_context(|| format!("open {} for trajectory recovery", path.display()))?;
        file.set_len(log.append_end)
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
    let mut reader = CompleteLineReader::new(BufReader::new(file), path.to_owned());
    let mut records = Vec::new();
    let mut record_count = 0_u64;
    let mut append_end = 0;
    let mut pending_tool_turn = None;
    while let Some(record) = reader.next_record().await? {
        record_count = record_count
            .checked_add(1)
            .context("committed message count overflow")?;
        update_pending_tool_turn(&mut pending_tool_turn, &record)?;
        append_end = record.end_offset;
        if collect_records {
            records.push(record.trajectory);
        }
    }
    debug_assert!(append_end <= reader.visible_end());
    if let Some(pending) = pending_tool_turn {
        append_end = pending.source_offset;
        record_count = pending.first_seq.saturating_sub(1);
    }

    Ok(JsonlFile {
        records,
        record_count,
        original_len: reader.bytes_read(),
        append_end,
    })
}

struct PendingToolTurn {
    source_offset: u64,
    first_seq: u64,
    call_ids: Vec<String>,
    next_result: usize,
}

fn update_pending_tool_turn(
    pending: &mut Option<PendingToolTurn>,
    record: &decoder::DecodedRecord,
) -> Result<()> {
    if let Some(turn) = pending.as_mut() {
        let [MessageContent::ToolResult { call_id, .. }] =
            record.trajectory.message.content.as_slice()
        else {
            anyhow::bail!(
                "message `{}` interrupts tool results for `{}`",
                record.trajectory.message_ref,
                turn.call_ids[turn.next_result]
            );
        };
        ensure!(
            record.trajectory.message.role == Role::Tool
                && *call_id == turn.call_ids[turn.next_result],
            "message `{}` returns tool call `{call_id}`, expected `{}`",
            record.trajectory.message_ref,
            turn.call_ids[turn.next_result]
        );
        turn.next_result += 1;
        if turn.next_result == turn.call_ids.len() {
            *pending = None;
        }
        return Ok(());
    }

    let call_ids = record
        .trajectory
        .message
        .tool_calls()
        .into_iter()
        .map(|call| call.id)
        .collect::<Vec<_>>();
    if !call_ids.is_empty() {
        *pending = Some(PendingToolTurn {
            source_offset: record.source_offset,
            first_seq: record.trajectory.seq,
            call_ids,
            next_result: 0,
        });
    }
    Ok(())
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
