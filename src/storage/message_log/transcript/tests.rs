use std::{
    fs::OpenOptions as StdOpenOptions,
    io::{Seek, SeekFrom, Write},
};

use chrono::Utc;
use fmtview::view::{
    RecordLoadLimit, RecordTimeline, TimelineRead, TimelineReadNext, TimelineRefresh,
    TimelineResetReason,
};
use tempfile::TempDir;

use super::*;
use crate::{
    artifact::ResultMetadata,
    model::{Message, MessageContent, Role, ToolArguments, ToolCall},
    storage::message_log::{LocalState, StoredMessage},
    trajectory::message_ref,
};

async fn test_store(state: RunState) -> (TempDir, RunDirStore) {
    let workspace = tempfile::tempdir().unwrap();
    let store = RunDirStore::new(workspace.path());
    let run = RunRecord::new(
        "run-1",
        "root",
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

fn raw_messages(first_seq: u64, messages: Vec<Message>) -> Vec<Vec<u8>> {
    messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            let mut raw = serde_json::to_vec(&StoredMessage {
                message_ref: message_ref(first_seq + index as u64),
                created_at: Utc::now(),
                message,
                local: LocalState::default(),
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
async fn opens_at_tail_and_moves_one_message_at_a_time() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    store
        .append_messages(
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
    let (previous, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(2, usize::MAX))
            .unwrap(),
    );
    assert_eq!(previous, ["m3", "m4"]);
    assert_eq!(next, TimelineReadNext::More);
}

#[tokio::test]
async fn complete_prefix_of_a_batch_is_immediately_visible() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    let records = raw_messages(
        1,
        vec![
            Message::text(Role::Assistant, "tool call"),
            Message::text(Role::User, "first result"),
            Message::text(Role::User, "second result"),
        ],
    );
    append_raw(&paths.messages, &records[..2]);

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let (visible, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(128, usize::MAX))
            .unwrap(),
    );
    assert_eq!(visible, ["m1", "m2"]);
    assert_eq!(next, TimelineReadNext::End);

    append_raw(&paths.messages, &records[2..]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));
    let (newer, next) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(128, usize::MAX))
            .unwrap(),
    );
    assert_eq!(newer, ["m3"]);
    assert_eq!(next, TimelineReadNext::Pending);
}

#[tokio::test]
async fn torn_line_stays_pending_until_its_newline_arrives() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let second = raw_messages(2, vec![Message::text(Role::Assistant, "second")])
        .pop()
        .unwrap();
    let split = second.len() / 2;
    append_raw(&paths.messages, &[second[..split].to_vec()]);

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    assert_eq!(timeline.snapshot().pending_bytes, split as u64);
    let (visible, _) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(8, usize::MAX))
            .unwrap(),
    );
    assert_eq!(visible, ["m1"]);

    append_raw(&paths.messages, &[second[split..].to_vec()]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));
    let (newer, _) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(8, usize::MAX))
            .unwrap(),
    );
    assert_eq!(newer, ["m2"]);
}

#[tokio::test]
async fn truncating_or_replacing_the_visible_prefix_resets_epoch() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    store
        .append_message("run-1", &Message::text(Role::User, "original"))
        .await
        .unwrap();
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();

    std::fs::write(
        &paths.messages,
        raw_messages(1, vec![Message::text(Role::User, "replacement")]).concat(),
    )
    .unwrap();
    let refresh = timeline.refresh().unwrap();
    assert!(matches!(
        refresh,
        TimelineRefresh::Reset {
            reason: TimelineResetReason::Replaced,
            ..
        }
    ));
    assert_eq!(timeline.snapshot().epoch, 2);

    std::fs::OpenOptions::new()
        .write(true)
        .open(&paths.messages)
        .unwrap()
        .set_len(0)
        .unwrap();
    let refresh = timeline.refresh().unwrap();
    assert!(matches!(
        refresh,
        TimelineRefresh::Reset {
            reason: TimelineResetReason::Truncated,
            ..
        }
    ));
}

#[tokio::test]
async fn terminal_state_turns_live_boundary_into_end() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "only"))
        .await
        .unwrap();
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    assert!(matches!(
        timeline.load_newer(RecordLoadLimit::new(8, 1024)).unwrap(),
        TimelineRead::Pending
    ));

    store
        .update_state("run-1", RunState::Completed)
        .await
        .unwrap();
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::End(_)
    ));
    assert!(matches!(
        timeline.load_newer(RecordLoadLimit::new(8, 1024)).unwrap(),
        TimelineRead::End
    ));
}

#[tokio::test]
async fn raw_tool_arguments_and_lf_are_preserved() {
    let (_workspace, store) = test_store(RunState::Completed).await;
    let arguments = "{\n  \"command\": \"printf 'a  b'\"\n}";
    store
        .append_message(
            "run-1",
            &Message::assistant(vec![MessageContent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: ToolArguments::from_raw(arguments),
            })]),
        )
        .await
        .unwrap();

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let TimelineRead::Records { records, .. } = timeline
        .load_older(RecordLoadLimit::new(8, usize::MAX))
        .unwrap()
    else {
        panic!("expected one record");
    };
    assert_eq!(records.len(), 1);
    assert!(records[0].raw.ends_with(b"\n"));
    let value: serde_json::Value = serde_json::from_slice(&records[0].raw).unwrap();
    assert_eq!(value["content"][0]["arguments"], arguments);
}

#[tokio::test]
async fn prefix_probe_is_bounded_and_does_not_advance_tail_cursor() {
    let (_workspace, store) = test_store(RunState::Completed).await;
    for seq in 1..=200 {
        store
            .append_message(
                "run-1",
                &Message::text(Role::User, format!("message-{seq}")),
            )
            .await
            .unwrap();
    }
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let (prefix, next) = record_refs(
        timeline
            .probe_prefix(RecordLoadLimit::new(2, usize::MAX))
            .unwrap(),
    );
    assert_eq!(prefix, ["m1", "m2"]);
    assert_eq!(next, TimelineReadNext::More);

    let (tail, _) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(tail, ["m200"]);
}

#[tokio::test]
async fn reverse_budget_returns_one_large_line_without_scanning_older_messages() {
    let (_workspace, store) = test_store(RunState::Completed).await;
    store
        .append_message("run-1", &Message::text(Role::User, "older"))
        .await
        .unwrap();
    store
        .append_message(
            "run-1",
            &Message::text(Role::Assistant, "x".repeat(512 * 1024)),
        )
        .await
        .unwrap();

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let before = timeline.instrumentation();
    let (tail, next) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    let after = timeline.instrumentation();
    assert_eq!(tail, ["m2"]);
    assert_eq!(next, TimelineReadNext::More);
    assert!(after.bytes_read.saturating_sub(before.bytes_read) < 2 * 1024 * 1024);
}

#[tokio::test]
async fn unchanged_torn_tail_is_not_rescanned_on_every_refresh() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    store
        .append_message("run-1", &Message::text(Role::User, "visible"))
        .await
        .unwrap();
    append_raw(&paths.messages, &[vec![b'x'; 512 * 1024]]);

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let before = timeline.instrumentation();
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Pending(_)
    ));
    let after_first = timeline.instrumentation();
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Pending(_)
    ));
    let after_second = timeline.instrumentation();
    assert!(after_first.bytes_read.saturating_sub(before.bytes_read) < 4 * 1024);
    assert!(
        after_second
            .bytes_read
            .saturating_sub(after_first.bytes_read)
            < 4 * 1024
    );
}

#[tokio::test]
async fn ndjson_writer_exposes_complete_lines_even_from_an_incomplete_tool_turn() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let partial_turn = raw_messages(
        2,
        vec![
            Message::assistant(vec![MessageContent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "true"}).into(),
            })]),
            Message::new(
                Role::Tool,
                vec![MessageContent::ToolResult {
                    call_id: "call-1".into(),
                    content: "done".into(),
                    is_error: false,
                    metadata: ResultMetadata::empty(),
                }],
            ),
        ],
    );
    append_raw(&paths.messages, &partial_turn[..1]);
    append_raw(
        &paths.messages,
        &[partial_turn[1][..partial_turn[1].len() - 1].to_vec()],
    );

    let mut output = Vec::new();
    store
        .write_complete_ndjson("run-1", &mut output)
        .await
        .unwrap();
    let refs = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice::<serde_json::Value>(line).unwrap()["ref"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(refs, ["m1", "m2"]);
}

#[tokio::test]
#[ignore = "million-record tail-open acceptance"]
async fn million_record_tail_open_reads_a_bounded_suffix() {
    let (_workspace, store) = test_store(RunState::Completed).await;
    let paths = store.paths("run-1");
    let mut file = StdOpenOptions::new()
        .write(true)
        .open(&paths.messages)
        .unwrap();
    for seq in 1..=1_000_000 {
        let record = raw_messages(seq, vec![Message::text(Role::User, "x")]);
        file.write_all(&record[0]).unwrap();
    }
    file.flush().unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();

    let timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    assert!(timeline.instrumentation().bytes_read < 512 * 1024);
}
