use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    model::openai_chat::{ChatMessage, project_chat_message},
    trajectory::{CompactionMessage, TrajectoryMessage},
};

use self::layout::ContentLayout;

mod layout;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageMetadata {
    pub(super) message_id: String,
    pub(super) seq: u64,
    pub(super) created_at: DateTime<Utc>,
    pub(super) message_sha256: String,
    layout: Vec<ContentLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionMessage>,
    reconstruction_sha256: String,
}

#[derive(Serialize)]
struct ReconstructionPayload<'a> {
    message_id: &'a str,
    seq: u64,
    created_at: DateTime<Utc>,
    message_sha256: &'a str,
    layout: &'a [ContentLayout],
    compaction: &'a Option<CompactionMessage>,
}

pub(super) struct EncodedMessage {
    pub(super) native_json: Vec<u8>,
    pub(super) metadata: MessageMetadata,
}

pub(super) fn encode(record: &TrajectoryMessage) -> Result<EncodedMessage> {
    let native = project_chat_message(&record.message);
    let layout = layout::encode(&record.message, &native)?;
    let native_json = serde_json::to_vec(&native).context("serialize OpenAI Chat message")?;
    let message_sha256 = sha256(&native_json);
    let reconstruction_sha256 = reconstruction_sha256(
        &record.message_ref,
        record.seq,
        record.created_at,
        &message_sha256,
        &layout,
        &record.compaction,
    )?;
    let metadata = MessageMetadata {
        message_id: record.message_ref.clone(),
        seq: record.seq,
        created_at: record.created_at,
        message_sha256,
        layout,
        compaction: record.compaction.clone(),
        reconstruction_sha256,
    };
    Ok(EncodedMessage {
        native_json,
        metadata,
    })
}

pub(super) fn decode(
    native: ChatMessage,
    native_json: &[u8],
    metadata: MessageMetadata,
    expected_seq: u64,
) -> Result<TrajectoryMessage> {
    ensure!(
        metadata.reconstruction_sha256
            == reconstruction_sha256(
                &metadata.message_id,
                metadata.seq,
                metadata.created_at,
                &metadata.message_sha256,
                &metadata.layout,
                &metadata.compaction,
            )?,
        "message {} reconstruction metadata does not match its sha256",
        metadata.message_id
    );
    ensure!(
        metadata.seq == expected_seq,
        "message metadata sequence {} is not the expected {expected_seq}",
        metadata.seq
    );
    ensure!(
        !metadata.message_id.is_empty(),
        "message metadata has an empty message id"
    );
    ensure!(
        metadata.message_sha256 == sha256(native_json),
        "message {} does not match its metadata sha256",
        metadata.message_id
    );

    let message = layout::decode(&native, metadata.layout)
        .with_context(|| format!("decode message {} layout", metadata.message_id))?;
    ensure!(
        project_chat_message(&message) == native,
        "message {} layout does not reproduce its OpenAI Chat message",
        metadata.message_id
    );
    Ok(TrajectoryMessage {
        message_ref: metadata.message_id,
        seq: metadata.seq,
        created_at: metadata.created_at,
        message,
        compaction: metadata.compaction,
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn reconstruction_sha256(
    message_id: &str,
    seq: u64,
    created_at: DateTime<Utc>,
    message_sha256: &str,
    layout: &[ContentLayout],
    compaction: &Option<CompactionMessage>,
) -> Result<String> {
    let payload = ReconstructionPayload {
        message_id,
        seq,
        created_at,
        message_sha256,
        layout,
        compaction,
    };
    Ok(sha256(
        &serde_json::to_vec(&payload).context("serialize message reconstruction metadata")?,
    ))
}
