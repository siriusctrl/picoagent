use std::path::Path;

use chrono::Utc;
use serde_json::json;
use tokio::io::BufReader;

use super::*;
use crate::{
    model::{Message, MessageContent, Role},
    storage::message_log::{LocalState, StoredMessage},
};

fn line(message_ref: &str, text: &str) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&StoredMessage {
        message_ref: message_ref.to_owned(),
        created_at: Utc::now(),
        message: Message::text(Role::User, text),
        local: LocalState::default(),
    })
    .unwrap();
    bytes.push(b'\n');
    bytes
}

#[tokio::test]
async fn every_complete_line_is_visible_and_a_torn_tail_is_not() {
    let first = line("m1", "first");
    let second = line("m2", "second");
    let mut bytes = first.clone();
    bytes.extend_from_slice(&second[..second.len() - 1]);
    let mut reader =
        CompleteLineReader::new(BufReader::new(bytes.as_slice()), "messages.jsonl".into());

    let first_record = reader.next_record().await.unwrap().unwrap();
    assert_eq!(first_record.trajectory.message_ref, "m1");
    assert_eq!(first_record.raw, first);
    assert_eq!(first_record.source_offset, 0);
    assert_eq!(first_record.end_offset, first.len() as u64);
    assert!(reader.next_record().await.unwrap().is_none());
    assert_eq!(reader.visible_end(), first.len() as u64);
    assert_eq!(reader.bytes_read(), bytes.len() as u64);
}

#[test]
fn decoder_preserves_raw_tool_arguments() {
    let arguments = "{\n  \"command\": \"printf 'a  b'\"\n}";
    let mut raw = serde_json::to_vec(&json!({
        "ref": "m1",
        "created_at": "2026-07-24T00:00:00Z",
        "role": "assistant",
        "content": [{
            "type": "tool_call",
            "id": "call_1",
            "name": "bash",
            "arguments": arguments,
        }],
    }))
    .unwrap();
    raw.push(b'\n');
    let expected = raw.clone();
    let mut decoder = LineDecoder::default();

    let record = decoder
        .push_complete_line(Path::new("messages.jsonl"), raw.clone(), raw.len() as u64)
        .unwrap();

    assert_eq!(record.raw, expected);
    let MessageContent::ToolCall(call) = &record.trajectory.message.content[0] else {
        panic!("stored content should remain a tool call");
    };
    assert_eq!(call.arguments.as_raw(), arguments);
}

#[test]
fn decoder_rejects_non_contiguous_refs_without_advancing() {
    let invalid = line("m2", "wrong");
    let valid = line("m1", "right");
    let mut decoder = LineDecoder::default();

    let error = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            invalid.clone(),
            invalid.len() as u64,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("message ref `m2` is not the expected `m1`")
    );

    let record = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            valid.clone(),
            valid.len() as u64,
        )
        .unwrap();
    assert_eq!(record.trajectory.message_ref, "m1");
    assert_eq!(decoder.next_seq(), 2);
}

#[test]
fn decoder_rejects_non_contiguous_offsets() {
    let raw = line("m1", "first");
    let mut decoder = LineDecoder::default();

    let error = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            raw.clone(),
            raw.len() as u64 + 1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("starts at byte 1, expected 0"));
}
