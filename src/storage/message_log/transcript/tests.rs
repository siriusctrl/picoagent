use std::{
    fs::OpenOptions as StdOpenOptions,
    io::{Seek, SeekFrom, Write},
};

use chrono::Utc;
use fmtview::view::{
    RecordLoadLimit, RecordTimeline, TimelineRead, TimelineRefresh, TimelineResetReason,
};
use tempfile::TempDir;

use super::*;
use crate::{
    artifact::ResultMetadata,
    model::{Message, MessageContent, Role, ToolArguments},
    storage::message_log::{LocalState, MessageCheckpoint, StoredMessage},
    trajectory::message_ref,
};

mod review;

async fn test_store(state: RunState) -> (TempDir, RunDirStore) {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let run = RunRecord::new(
        "run-1",
        "inspect",
        "test-provider",
        "test-model",
        workspace.path().to_owned(),
        None,
    );
    store.create_run(&run).await.unwrap();
    store.update_state("run-1", state).await.unwrap();
    (workspace, store)
}

fn raw_checkpoint(first_seq: u64, messages: Vec<Message>) -> Vec<Vec<u8>> {
    let first_message_ref = message_ref(first_seq);
    let count = messages.len() as u64;
    messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            let mut raw = serde_json::to_vec(&StoredMessage {
                message_ref: message_ref(first_seq + index as u64),
                created_at: Utc::now(),
                message,
                local: LocalState {
                    checkpoint: Some(MessageCheckpoint {
                        first_message_ref: first_message_ref.clone(),
                        index: index as u64,
                        count,
                    }),
                    pending_input_id: None,
                    compaction: None,
                },
            })
            .unwrap();
            raw.push(b'\n');
            raw
        })
        .collect()
}

fn append_raw(path: &Path, records: &[Vec<u8>]) {
    let mut file = StdOpenOptions::new().append(true).open(path).unwrap();
    for record in records {
        file.write_all(record).unwrap();
    }
    file.flush().unwrap();
}

fn record_refs(read: TimelineRead) -> (Vec<String>, TimelineReadNext) {
    match read {
        TimelineRead::Records { records, next } => (
            records
                .into_iter()
                .map(|record| {
                    serde_json::from_slice::<serde_json::Value>(&record.raw).unwrap()["ref"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect(),
            next,
        ),
        other => panic!("expected records, got {other:?}"),
    }
}

#[tokio::test]
async fn opens_at_tail_and_moves_by_whole_checkpoints_in_source_order() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    store
        .append_checkpoint(
            "run-1",
            &[
                Message::text(Role::Assistant, "call"),
                Message::text(Role::User, "result one"),
                Message::text(Role::User, "result two"),
            ],
        )
        .await
        .unwrap();
    store
        .append_message("run-1", &Message::text(Role::Assistant, "last"))
        .await
        .unwrap();

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let (tail, next) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(tail, ["m5"]);
    assert_eq!(next, TimelineReadNext::More);

    let (checkpoint, next) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(checkpoint, ["m2", "m3", "m4"]);
    assert_eq!(next, TimelineReadNext::More);

    let (first, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
    );
    assert_eq!(first, ["m1"]);
    assert_eq!(next, TimelineReadNext::End);
}

#[tokio::test]
async fn ignores_torn_lines_and_complete_but_incomplete_checkpoints() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "committed"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let incomplete = raw_checkpoint(
        2,
        vec![
            Message::text(Role::Assistant, "call"),
            Message::text(Role::User, "result"),
        ],
    );
    append_raw(&paths.messages, &incomplete[..1]);
    let mut file = StdOpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .unwrap();
    file.write_all(b"{\"ref\":\"m3\"").unwrap();
    file.flush().unwrap();

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    assert!(timeline.snapshot().pending_bytes > 0);
    let (records, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
    );
    assert_eq!(records, ["m1"]);
    assert_eq!(next, TimelineReadNext::End);
}

#[tokio::test]
async fn pending_append_becomes_one_atomic_newer_batch() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let checkpoint = raw_checkpoint(
        2,
        vec![
            Message::text(Role::Assistant, "call"),
            Message::text(Role::User, "result"),
        ],
    );
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    append_raw(&paths.messages, &checkpoint[..1]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Pending(_)
    ));
    assert!(matches!(
        timeline
            .load_newer(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
        TimelineRead::Pending
    ));

    append_raw(&paths.messages, &checkpoint[1..]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));
    let (records, next) = record_refs(timeline.load_newer(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(records, ["m2", "m3"]);
    assert_eq!(next, TimelineReadNext::Pending);
}

#[tokio::test]
async fn replacing_only_the_uncommitted_tail_does_not_reset_or_repeat() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let committed_end = std::fs::metadata(&paths.messages).unwrap().len();
    let discarded = raw_checkpoint(
        2,
        vec![
            Message::text(Role::Assistant, "discarded"),
            Message::text(Role::User, "missing"),
        ],
    );
    let replacement = raw_checkpoint(2, vec![Message::text(Role::Assistant, "replacement")]);
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    append_raw(&paths.messages, &discarded[..1]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Pending(_)
    ));

    StdOpenOptions::new()
        .write(true)
        .open(&paths.messages)
        .unwrap()
        .set_len(committed_end)
        .unwrap();
    append_raw(&paths.messages, &replacement);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));
    assert_eq!(timeline.snapshot().epoch, 1);
    let (records, _) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
    );
    assert_eq!(records, ["m2"]);
    assert!(matches!(
        timeline
            .load_newer(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
        TimelineRead::Pending
    ));
}

#[tokio::test]
async fn truncating_or_replacing_the_committed_prefix_resets_epoch() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let mut truncated = TranscriptTimeline::open(&store, "run-1").unwrap();
    StdOpenOptions::new()
        .write(true)
        .open(&paths.messages)
        .unwrap()
        .set_len(0)
        .unwrap();
    assert!(matches!(
        truncated.refresh().unwrap(),
        TimelineRefresh::Reset {
            reason: TimelineResetReason::Truncated,
            ..
        }
    ));
    assert_eq!(truncated.snapshot().epoch, 2);

    append_raw(
        &paths.messages,
        &raw_checkpoint(1, vec![Message::text(Role::User, "replacement")]),
    );
    let mut replaced = TranscriptTimeline::open(&store, "run-1").unwrap();
    let temporary = paths.messages.with_extension("new");
    std::fs::write(
        &temporary,
        raw_checkpoint(1, vec![Message::text(Role::User, "new inode")]).concat(),
    )
    .unwrap();
    std::fs::rename(temporary, &paths.messages).unwrap();
    assert!(matches!(
        replaced.refresh().unwrap(),
        TimelineRefresh::Reset {
            reason: TimelineResetReason::IdentityChanged,
            ..
        }
    ));
}

#[tokio::test]
async fn same_inode_middle_rewrite_resets_as_replaced() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message(
            "run-1",
            &Message::text(Role::User, format!("before{}after", "a".repeat(2048))),
        )
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let bytes = std::fs::read(&paths.messages).unwrap();
    let middle = bytes.len() / 2;
    assert_eq!(bytes[middle], b'a');
    let mut file = StdOpenOptions::new()
        .write(true)
        .open(&paths.messages)
        .unwrap();
    file.seek(SeekFrom::Start(middle as u64)).unwrap();
    file.write_all(b"b").unwrap();
    file.flush().unwrap();

    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Reset {
            reason: TimelineResetReason::Replaced,
            ..
        }
    ));
}

#[tokio::test]
async fn terminal_state_turns_live_boundaries_into_end() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    store
        .update_state("run-1", RunState::Completed)
        .await
        .unwrap();
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::End(_)
    ));
    assert!(matches!(
        timeline
            .load_newer(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
        TimelineRead::End
    ));
}

#[tokio::test]
async fn raw_tool_arguments_and_lf_are_preserved() {
    let (_workspace, store) = test_store(RunState::Completed).await;
    let arguments = "{\n  \"cmd\": \"cargo test\"  \n}";
    store
        .append_message(
            "run-1",
            &Message {
                role: Role::Assistant,
                content: vec![MessageContent::ToolCall {
                    id: "call-1".to_owned(),
                    name: "bash".to_owned(),
                    arguments: ToolArguments::from_raw(arguments),
                }],
            },
        )
        .await
        .unwrap();
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let read = timeline
        .load_older(RecordLoadLimit::new(128, 1024 * 1024))
        .unwrap();
    let TimelineRead::Records { records, .. } = read else {
        panic!("expected records");
    };
    assert!(records[0].raw.ends_with(b"\n"));
    let value: serde_json::Value = serde_json::from_slice(&records[0].raw).unwrap();
    assert_eq!(value["content"][0]["arguments"], arguments);
}

#[tokio::test]
async fn prefix_probe_is_bounded_and_does_not_advance_tail_cursors() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    let records = (1..=5000)
        .flat_map(|seq| raw_checkpoint(seq, vec![Message::text(Role::User, "small")]))
        .collect::<Vec<_>>();
    append_raw(&paths.messages, &records);
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let before = timeline.instrumentation();
    let (prefix, _) = record_refs(
        timeline
            .probe_prefix(RecordLoadLimit::new(2, 1024))
            .unwrap(),
    );
    let after = timeline.instrumentation();
    assert_eq!(prefix, ["m1", "m2"]);
    assert!(after.bytes_read - before.bytes_read < 4096);
    let (tail, _) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(tail, ["m5000"]);
}

#[tokio::test]
async fn reverse_budget_does_not_decode_a_huge_checkpoint_before_a_small_tail() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    let large = raw_checkpoint(
        1,
        (0..1024)
            .map(|index| {
                Message::text(Role::User, format!("LARGE_{index:04}_{}", "x".repeat(4096)))
            })
            .collect(),
    );
    append_raw(&paths.messages, &large);
    append_raw(
        &paths.messages,
        &raw_checkpoint(1025, vec![Message::text(Role::Assistant, "small tail")]),
    );
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();

    let before = timeline.instrumentation();
    let (tail, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(128, 4 * 1024 * 1024))
            .unwrap(),
    );
    let after_tail = timeline.instrumentation();
    assert_eq!(tail, ["m1025"]);
    assert_eq!(next, TimelineReadNext::More);
    assert!(after_tail.bytes_read - before.bytes_read < 256 * 1024);

    let (large_group, next) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    let after_large = timeline.instrumentation();
    assert_eq!(large_group.len(), 1024);
    assert_eq!(large_group.first().unwrap(), "m1");
    assert_eq!(large_group.last().unwrap(), "m1024");
    assert_eq!(next, TimelineReadNext::End);
    assert!(after_large.bytes_read - after_tail.bytes_read > 4 * 1024 * 1024);
}

#[tokio::test]
async fn unchanged_large_incomplete_checkpoint_is_not_rescanned_each_refresh() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    let checkpoint = raw_checkpoint(
        1,
        (0..4000)
            .map(|index| Message::text(Role::User, format!("pending-{index}")))
            .collect(),
    );
    append_raw(&paths.messages, &checkpoint[..checkpoint.len() - 1]);
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let before = timeline.instrumentation();
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Pending(_)
    ));
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Pending(_)
    ));
    let unchanged = timeline.instrumentation();
    assert!(unchanged.bytes_read - before.bytes_read < 1024);

    append_raw(&paths.messages, &checkpoint[checkpoint.len() - 1..]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));
    let appended = timeline.instrumentation();
    assert!(appended.bytes_read - unchanged.bytes_read < 4096);
    let (records, _) = record_refs(timeline.load_newer(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(records.len(), 4000);
}

#[tokio::test]
async fn ndjson_writer_exposes_only_complete_checkpoints() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "committed"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let incomplete = raw_checkpoint(
        2,
        vec![
            Message::text(Role::Assistant, "call"),
            Message {
                role: Role::Tool,
                content: vec![MessageContent::ToolResult {
                    call_id: "call-1".to_owned(),
                    content: "result".to_owned(),
                    is_error: false,
                    metadata: ResultMetadata::empty(),
                }],
            },
        ],
    );
    append_raw(&paths.messages, &incomplete[..1]);
    let mut output = Vec::new();
    store
        .write_committed_ndjson("run-1", &mut output)
        .await
        .unwrap();
    assert_eq!(
        output,
        std::fs::read(&paths.messages).unwrap()[..output.len()]
    );
    assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
}

#[tokio::test]
#[ignore = "million-record tail-open acceptance"]
async fn million_record_tail_open_reads_a_bounded_suffix() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    let mut file = StdOpenOptions::new()
        .append(true)
        .open(&paths.messages)
        .unwrap();
    for seq in 1..=1_000_000_u64 {
        let record = raw_checkpoint(seq, vec![Message::text(Role::User, "x")]);
        file.write_all(&record[0]).unwrap();
    }
    file.flush().unwrap();
    let timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    assert_eq!(timeline.committed_next_seq, 1_000_001);
    assert!(timeline.instrumentation().bytes_read < 512 * 1024);
}
