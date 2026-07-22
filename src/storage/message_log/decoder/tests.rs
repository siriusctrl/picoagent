use std::{
    io::Cursor,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use serde_json::json;
use tokio::io::{AsyncRead, BufReader, ReadBuf};

use crate::model::MessageContent;

use super::*;

fn line(message_ref: &str, index: u64, count: u64, text: &str) -> Vec<u8> {
    let first = message_ref
        .strip_prefix('m')
        .and_then(|seq| seq.parse::<u64>().ok())
        .unwrap()
        .saturating_sub(index);
    let mut bytes = serde_json::to_vec(&json!({
        "ref": message_ref,
        "created_at": "2026-07-22T00:00:00Z",
        "role": "user",
        "content": [{"type": "text", "text": text}],
        "_fiasco": {"checkpoint": {
            "first_message_ref": format!("m{first}"),
            "index": index,
            "count": count,
        }},
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

async fn collect_visible(bytes: Vec<u8>) -> Vec<String> {
    let mut reader = CommittedCheckpointReader::new(
        BufReader::with_capacity(31, Cursor::new(bytes)),
        PathBuf::from("messages.jsonl"),
    );
    let mut refs = Vec::new();
    loop {
        match reader.next_checkpoint().await.unwrap() {
            DecodeResult::Checkpoint(checkpoint) => refs.extend(
                checkpoint
                    .records
                    .into_iter()
                    .map(|record| record.trajectory.message_ref),
            ),
            DecodeResult::NeedMore => return refs,
        }
    }
}

#[tokio::test]
async fn every_torn_tail_cut_hides_the_incomplete_checkpoint() {
    let prefix = line("m1", 0, 1, "prefix");
    let group = [
        line("m2", 0, 3, "assistant"),
        line("m3", 1, 3, "result one"),
        line("m4", 2, 3, "result two"),
    ]
    .concat();
    let complete = [prefix.as_slice(), group.as_slice()].concat();

    for cut in 0..=complete.len() {
        let visible = collect_visible(complete[..cut].to_vec()).await;
        let expected = if cut < prefix.len() {
            Vec::new()
        } else if cut < complete.len() {
            vec!["m1".to_owned()]
        } else {
            vec![
                "m1".to_owned(),
                "m2".to_owned(),
                "m3".to_owned(),
                "m4".to_owned(),
            ]
        };
        assert_eq!(visible, expected, "unexpected visible prefix at cut {cut}");
    }
}

#[test]
fn decoder_buffers_only_the_current_checkpoint_and_preserves_raw_line() {
    let first = line("m1", 0, 2, "first");
    let second = line("m2", 1, 2, "second");
    let mut decoder = CheckpointDecoder::default();

    assert!(matches!(
        decoder
            .push_complete_line(
                Path::new("messages.jsonl"),
                first.clone(),
                first.len() as u64
            )
            .unwrap(),
        DecodeResult::NeedMore
    ));
    let pending = decoder.pending.as_ref().unwrap();
    assert_eq!(pending.records.len(), 1);
    assert_eq!(pending.records[0].raw, first);
    assert_eq!(pending.records[0].source_offset, 0);

    let end = (first.len() + second.len()) as u64;
    let DecodeResult::Checkpoint(checkpoint) = decoder
        .push_complete_line(Path::new("messages.jsonl"), second.clone(), end)
        .unwrap()
    else {
        panic!("second record should complete the checkpoint");
    };
    assert!(decoder.pending.is_none());
    assert_eq!(checkpoint.records.len(), 2);
    assert_eq!(checkpoint.records[1].raw, second);
    assert_eq!(checkpoint.records[1].source_offset, first.len() as u64);
    assert_eq!(checkpoint.committed_end, end);
}

#[test]
fn decoder_validates_a_candidate_group_from_a_tail_offset() {
    let first = line("m900", 0, 2, "tail call");
    let second = line("m901", 1, 2, "tail result");
    let source_offset = 8_000_000_u64;
    let mut decoder = CheckpointDecoder::new(900, source_offset);

    assert!(matches!(
        decoder
            .push_complete_line(
                Path::new("messages.jsonl"),
                first.clone(),
                source_offset + first.len() as u64,
            )
            .unwrap(),
        DecodeResult::NeedMore
    ));
    let end = source_offset + (first.len() + second.len()) as u64;
    let DecodeResult::Checkpoint(checkpoint) = decoder
        .push_complete_line(Path::new("messages.jsonl"), second, end)
        .unwrap()
    else {
        panic!("candidate tail group should validate independently");
    };
    assert_eq!(
        checkpoint
            .records
            .iter()
            .map(|record| record.trajectory.message_ref.as_str())
            .collect::<Vec<_>>(),
        ["m900", "m901"]
    );
    assert_eq!(checkpoint.records[0].source_offset, source_offset);
    assert_eq!(checkpoint.committed_end, end);
}

#[test]
fn malformed_or_inconsistent_completed_lines_fail_without_publishing_the_group() {
    let first = line("m1", 0, 2, "first");
    let mut decoder = CheckpointDecoder::default();
    assert!(matches!(
        decoder
            .push_complete_line(
                Path::new("messages.jsonl"),
                first.clone(),
                first.len() as u64,
            )
            .unwrap(),
        DecodeResult::NeedMore
    ));

    let error = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            b"{not-json}\n".to_vec(),
            first.len() as u64 + 11,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("parse completed message"));
    assert_eq!(decoder.pending.as_ref().unwrap().records.len(), 1);

    let inconsistent = line("m2", 0, 1, "wrong checkpoint");
    let error = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            inconsistent.clone(),
            (first.len() + inconsistent.len()) as u64,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("inconsistent checkpoint metadata")
    );
    assert_eq!(decoder.pending.as_ref().unwrap().records.len(), 1);
}

#[test]
fn committed_record_keeps_exact_json_and_tool_argument_string() {
    let arguments = "{\n  \"cmd\": \"printf 'a  b'\"\n}";
    let mut raw = serde_json::to_vec(&json!({
        "ref": "m1",
        "created_at": "2026-07-22T00:00:00Z",
        "role": "assistant",
        "content": [{
            "type": "tool_call",
            "id": "call_1",
            "name": "bash",
            "arguments": arguments,
        }],
        "_fiasco": {"checkpoint": {
            "first_message_ref": "m1",
            "index": 0,
            "count": 1,
        }},
    }))
    .unwrap();
    raw.push(b'\n');
    let expected_raw = raw.clone();
    let mut decoder = CheckpointDecoder::default();

    let DecodeResult::Checkpoint(checkpoint) = decoder
        .push_complete_line(Path::new("messages.jsonl"), raw.clone(), raw.len() as u64)
        .unwrap()
    else {
        panic!("singleton checkpoint should commit");
    };
    assert_eq!(checkpoint.records[0].raw, expected_raw);
    let MessageContent::ToolCall {
        arguments: decoded, ..
    } = &checkpoint.records[0].trajectory.message.content[0]
    else {
        panic!("stored content should remain a tool call");
    };
    assert_eq!(decoded.as_raw(), arguments);
}

struct CountingReader<R> {
    inner: R,
    reads: Arc<AtomicUsize>,
}

impl<R: AsyncRead + Unpin> AsyncRead for CountingReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(context, buffer);
        if let Poll::Ready(Ok(())) = &poll {
            self.reads
                .fetch_add(buffer.filled().len() - before, Ordering::Relaxed);
        }
        poll
    }
}

#[tokio::test]
async fn first_checkpoint_does_not_read_the_rest_of_a_large_log() {
    let mut bytes = Vec::new();
    for seq in 1..=20_000_u64 {
        bytes.extend(line(&format!("m{seq}"), 0, 1, "payload"));
    }
    let total = bytes.len();
    let reads = Arc::new(AtomicUsize::new(0));
    let source = CountingReader {
        inner: Cursor::new(bytes),
        reads: reads.clone(),
    };
    let mut reader = CommittedCheckpointReader::new(
        BufReader::with_capacity(256, source),
        PathBuf::from("messages.jsonl"),
    );

    let DecodeResult::Checkpoint(first) = reader.next_checkpoint().await.unwrap() else {
        panic!("first complete line should produce a checkpoint");
    };
    assert_eq!(first.records[0].trajectory.message_ref, "m1");
    assert!(reads.load(Ordering::Relaxed) < total / 100);
    assert!(reader.bytes_read() < (total / 100) as u64);
}

#[tokio::test]
async fn partial_line_can_complete_after_a_later_read() {
    let complete = line("m1", 0, 1, "later");
    let split = complete.len() / 2;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("messages.jsonl");
    tokio::fs::write(&path, &complete[..split]).await.unwrap();
    let file = tokio::fs::File::open(&path).await.unwrap();
    let mut reader = CommittedCheckpointReader::new(BufReader::new(file), path.clone());

    assert!(matches!(
        reader.next_checkpoint().await.unwrap(),
        DecodeResult::NeedMore
    ));
    use tokio::io::AsyncWriteExt;
    let mut writer = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    writer.write_all(&complete[split..]).await.unwrap();
    writer.flush().await.unwrap();
    let DecodeResult::Checkpoint(checkpoint) = reader.next_checkpoint().await.unwrap() else {
        panic!("completed line should become visible");
    };
    assert_eq!(checkpoint.records[0].raw, complete);
}
