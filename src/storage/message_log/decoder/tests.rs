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
fn decoder_does_not_reserve_the_declared_checkpoint_count() {
    let first = line("m1", 0, 1_000_000, "first");
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
    let pending = decoder.pending.as_ref().unwrap();
    assert_eq!(pending.records.len(), 1);
    assert!(pending.records.capacity() < 16);
}

#[test]
fn sequence_overflow_rejects_checkpoint_before_advancing_decoder_state() {
    let initial_seq = u64::MAX - 1;
    let initial_offset = 97_u64;
    let overflowing = line(&format!("m{initial_seq}"), 0, 2, "first");
    let overflowing_end = initial_offset + overflowing.len() as u64;
    let mut decoder = CheckpointDecoder::new(initial_seq, initial_offset);

    let error = decoder
        .push_complete_line(Path::new("messages.jsonl"), overflowing, overflowing_end)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("message sequence overflow after checkpoint")
    );
    assert_eq!(decoder.next_seq, initial_seq);
    assert_eq!(decoder.committed_end, initial_offset);
    assert_eq!(decoder.next_line_offset, initial_offset);
    assert!(decoder.pending.is_none());

    let valid = line(&format!("m{initial_seq}"), 0, 1, "retry");
    let valid_end = initial_offset + valid.len() as u64;
    let DecodeResult::Checkpoint(checkpoint) = decoder
        .push_complete_line(Path::new("messages.jsonl"), valid, valid_end)
        .unwrap()
    else {
        panic!("decoder should remain reusable after rejecting the overflow");
    };
    assert_eq!(checkpoint.committed_end, valid_end);
    assert_eq!(decoder.next_seq, u64::MAX);
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

struct DeterministicChunkReader {
    bytes: Vec<u8>,
    offset: usize,
    step: usize,
}

impl AsyncRead for DeterministicChunkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.offset == self.bytes.len() {
            return Poll::Ready(Ok(()));
        }
        let chunk = (self.step.wrapping_mul(17).wrapping_add(5) % 23) + 1;
        self.step = self.step.wrapping_add(1);
        let end = self
            .offset
            .saturating_add(chunk.min(buffer.remaining()))
            .min(self.bytes.len());
        buffer.put_slice(&self.bytes[self.offset..end]);
        self.offset = end;
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn deterministic_chunk_boundaries_preserve_checkpoint_and_raw_order() {
    let expected = [
        line("m1", 0, 1, "prefix"),
        line("m2", 0, 3, "assistant"),
        line("m3", 1, 3, "first result"),
        line("m4", 2, 3, "second result"),
        line("m5", 0, 1, "suffix"),
    ]
    .concat();
    let source = DeterministicChunkReader {
        bytes: expected.clone(),
        offset: 0,
        step: 0,
    };
    let mut reader = CommittedCheckpointReader::new(
        BufReader::with_capacity(29, source),
        PathBuf::from("messages.jsonl"),
    );
    let mut refs = Vec::new();
    let mut raw = Vec::new();
    while let DecodeResult::Checkpoint(checkpoint) = reader.next_checkpoint().await.unwrap() {
        for record in checkpoint.records {
            refs.push(record.trajectory.message_ref);
            raw.extend(record.raw);
        }
    }

    assert_eq!(refs, ["m1", "m2", "m3", "m4", "m5"]);
    assert_eq!(raw, expected);
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

#[tokio::test]
async fn partial_multi_record_checkpoint_commits_once_after_multiple_eofs() {
    let prefix = line("m1", 0, 1, "prefix");
    let first = line("m2", 0, 3, "assistant");
    let second = line("m3", 1, 3, "first result");
    let third = line("m4", 2, 3, "second result");
    let second_split = second.len() / 2;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("messages.jsonl");
    let initial = [prefix.as_slice(), first.as_slice(), &second[..second_split]].concat();
    tokio::fs::write(&path, &initial).await.unwrap();
    let file = tokio::fs::File::open(&path).await.unwrap();
    let mut reader = CommittedCheckpointReader::new(BufReader::new(file), path.clone());

    let DecodeResult::Checkpoint(committed_prefix) = reader.next_checkpoint().await.unwrap() else {
        panic!("the singleton prefix should be committed");
    };
    assert_eq!(committed_prefix.records[0].trajectory.message_ref, "m1");
    assert_eq!(reader.committed_end(), prefix.len() as u64);

    assert!(matches!(
        reader.next_checkpoint().await.unwrap(),
        DecodeResult::NeedMore
    ));
    assert!(matches!(
        reader.next_checkpoint().await.unwrap(),
        DecodeResult::NeedMore
    ));
    assert_eq!(reader.committed_end(), prefix.len() as u64);

    use tokio::io::AsyncWriteExt;
    let mut writer = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .unwrap();
    writer.write_all(&second[second_split..]).await.unwrap();
    writer.flush().await.unwrap();
    assert!(matches!(
        reader.next_checkpoint().await.unwrap(),
        DecodeResult::NeedMore
    ));
    assert!(matches!(
        reader.next_checkpoint().await.unwrap(),
        DecodeResult::NeedMore
    ));
    assert_eq!(reader.committed_end(), prefix.len() as u64);

    writer.write_all(&third).await.unwrap();
    writer.flush().await.unwrap();
    let DecodeResult::Checkpoint(checkpoint) = reader.next_checkpoint().await.unwrap() else {
        panic!("the completed group should commit after the final append");
    };
    assert_eq!(
        checkpoint
            .records
            .iter()
            .map(|record| record.trajectory.message_ref.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m3", "m4"]
    );
    assert_eq!(checkpoint.records[0].source_offset, prefix.len() as u64);
    assert_eq!(
        checkpoint.committed_end,
        (prefix.len() + first.len() + second.len() + third.len()) as u64
    );
    assert!(matches!(
        reader.next_checkpoint().await.unwrap(),
        DecodeResult::NeedMore
    ));
}

#[test]
fn decoder_rejects_non_contiguous_and_underflowing_line_offsets() {
    let first = line("m1", 0, 1, "first");
    let mut decoder = CheckpointDecoder::default();
    let error = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            first.clone(),
            first.len() as u64 + 7,
        )
        .unwrap_err();
    assert!(error.to_string().contains("starts at byte 7, expected 0"));

    let error = decoder
        .push_complete_line(Path::new("messages.jsonl"), first.clone(), 1)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("record offset precedes the start of the file")
    );

    let DecodeResult::Checkpoint(_) = decoder
        .push_complete_line(
            Path::new("messages.jsonl"),
            first.clone(),
            first.len() as u64,
        )
        .unwrap()
    else {
        panic!("the contiguous first line should commit");
    };
    let second = line("m2", 0, 1, "second");
    let overlap_end = first.len() as u64 + second.len() as u64 - 1;
    let error = decoder
        .push_complete_line(Path::new("messages.jsonl"), second, overlap_end)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "message record starts at byte {}, expected {}",
            first.len() - 1,
            first.len()
        )
    );
}
