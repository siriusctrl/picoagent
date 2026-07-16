use std::{collections::HashSet, io::SeekFrom, path::Path};

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use tokio::{
    fs::OpenOptions,
    io::{AsyncSeekExt, AsyncWriteExt},
};

use crate::{model::openai_chat::ChatMessage, trajectory::TrajectoryMessage};

use self::codec::MessageMetadata;

mod codec;

const LOCK_FILE: &str = ".message-log.lock";

pub(super) struct MessageLogLock {
    _file: std::fs::File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LogLengths {
    pub(super) messages: u64,
    pub(super) metadata: u64,
}

struct JsonlRecord<T> {
    value: T,
    raw: Vec<u8>,
    start: u64,
}

struct JsonlFile<T> {
    records: Vec<JsonlRecord<T>>,
    original_len: u64,
    valid_end: u64,
    needs_newline: bool,
}

pub(super) async fn initialize(
    run_directory: &Path,
    messages_path: &Path,
    metadata_path: &Path,
) -> Result<()> {
    for path in [messages_path, metadata_path, &run_directory.join(LOCK_FILE)] {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("create message log file {}", path.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("sync message log file {}", path.display()))?;
    }
    sync_directory(run_directory).await
}

pub(super) async fn exclusive_lock(run_directory: &Path) -> Result<MessageLogLock> {
    let path = run_directory.join(LOCK_FILE);
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open message log lock {}", path.display()))?;
        file.lock()
            .with_context(|| format!("lock message log {}", path.display()))?;
        Ok(MessageLogLock { _file: file })
    })
    .await
    .context("join message log lock task")?
}

pub(super) async fn lengths(messages_path: &Path, metadata_path: &Path) -> Result<LogLengths> {
    Ok(LogLengths {
        messages: optional_len(messages_path).await?,
        metadata: optional_len(metadata_path).await?,
    })
}

pub(super) async fn append(
    messages_path: &Path,
    metadata_path: &Path,
    record: &TrajectoryMessage,
) -> Result<()> {
    let encoded = codec::encode(record)?;
    append_line_sync(messages_path, &encoded.native_json).await?;
    let metadata =
        serde_json::to_vec(&encoded.metadata).context("serialize local message metadata")?;
    append_line_sync(metadata_path, &metadata).await
}

pub(super) async fn load(
    messages_path: &Path,
    metadata_path: &Path,
) -> Result<Vec<TrajectoryMessage>> {
    let messages = read_jsonl::<ChatMessage>(messages_path, "OpenAI Chat message").await?;
    let metadata = read_jsonl::<MessageMetadata>(metadata_path, "message metadata").await?;
    decode_committed(&messages, &metadata)
}

pub(super) async fn prepare_append(messages_path: &Path, metadata_path: &Path) -> Result<u64> {
    let messages = read_jsonl::<ChatMessage>(messages_path, "OpenAI Chat message").await?;
    let metadata = read_jsonl::<MessageMetadata>(metadata_path, "message metadata").await?;
    let committed = decode_committed(&messages, &metadata)?.len();

    let message_target = if messages.records.len() == metadata.records.len() + 1 {
        messages.records[committed].start
    } else {
        messages.valid_end
    };
    normalize_jsonl(
        messages_path,
        messages.original_len,
        message_target,
        message_target == messages.valid_end && messages.needs_newline,
    )
    .await?;
    normalize_jsonl(
        metadata_path,
        metadata.original_len,
        metadata.valid_end,
        metadata.needs_newline,
    )
    .await?;
    Ok(committed as u64)
}

fn decode_committed(
    messages: &JsonlFile<ChatMessage>,
    metadata: &JsonlFile<MessageMetadata>,
) -> Result<Vec<TrajectoryMessage>> {
    ensure!(
        metadata.records.len() <= messages.records.len(),
        "message metadata is ahead of messages.jsonl"
    );
    ensure!(
        messages.records.len() <= metadata.records.len() + 1,
        "messages.jsonl contains more than one uncommitted message"
    );

    let mut message_ids = HashSet::new();
    messages
        .records
        .iter()
        .zip(&metadata.records)
        .enumerate()
        .map(|(index, (message, metadata))| {
            let decoded = codec::decode(
                message.value.clone(),
                &message.raw,
                metadata.value.clone(),
                index as u64 + 1,
            )?;
            ensure!(
                message_ids.insert(decoded.message_ref.clone()),
                "duplicate message id {}",
                decoded.message_ref
            );
            Ok(decoded)
        })
        .collect()
}

async fn read_jsonl<T: DeserializeOwned>(path: &Path, record_name: &str) -> Result<JsonlFile<T>> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read initialized message log {}", path.display()))?;
    let original_len = bytes.len() as u64;
    let mut records = Vec::new();
    let mut offset = 0_usize;

    for line_with_newline in bytes.split_inclusive(|byte| *byte == b'\n') {
        let has_newline = line_with_newline.ends_with(b"\n");
        let line = if has_newline {
            &line_with_newline[..line_with_newline.len() - 1]
        } else {
            line_with_newline
        };
        if !has_newline {
            break;
        }
        ensure!(
            !line.iter().all(u8::is_ascii_whitespace),
            "blank line in {}",
            path.display()
        );
        let value = serde_json::from_slice(line)
            .with_context(|| format!("parse completed {record_name} in {}", path.display()))?;
        records.push(JsonlRecord {
            value,
            raw: line.to_vec(),
            start: offset as u64,
        });
        offset += line_with_newline.len();
    }

    let tail = &bytes[offset..];
    let (valid_end, needs_newline) = if tail.is_empty() || tail.iter().all(u8::is_ascii_whitespace)
    {
        (offset as u64, false)
    } else {
        match serde_json::from_slice(tail) {
            Ok(value) => {
                records.push(JsonlRecord {
                    value,
                    raw: tail.to_vec(),
                    start: offset as u64,
                });
                (original_len, true)
            }
            Err(_) => (offset as u64, false),
        }
    };

    Ok(JsonlFile {
        records,
        original_len,
        valid_end,
        needs_newline,
    })
}

async fn normalize_jsonl(
    path: &Path,
    original_len: u64,
    target_len: u64,
    add_newline: bool,
) -> Result<()> {
    if original_len == target_len && !add_newline {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(false)
        .open(path)
        .await
        .with_context(|| format!("open {} for trajectory recovery", path.display()))?;
    file.set_len(target_len)
        .await
        .with_context(|| format!("truncate {} for trajectory recovery", path.display()))?;
    if add_newline {
        file.seek(SeekFrom::End(0)).await?;
        file.write_all(b"\n").await?;
    }
    file.flush().await?;
    file.sync_data()
        .await
        .with_context(|| format!("sync recovered trajectory file {}", path.display()))
}

async fn append_line_sync(path: &Path, value: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {} for append", path.display()))?;
    file.write_all(value)
        .await
        .with_context(|| format!("append {}", path.display()))?;
    file.write_all(b"\n").await?;
    file.flush().await?;
    file.sync_data()
        .await
        .with_context(|| format!("sync {}", path.display()))
}

async fn optional_len(path: &Path) -> Result<u64> {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.len())
        .with_context(|| format!("inspect initialized message log {}", path.display()))
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
