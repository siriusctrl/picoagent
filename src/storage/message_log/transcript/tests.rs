use std::{fs::OpenOptions as StdOpenOptions, io::Write};

use chrono::Utc;
use fmtview::view::{
    RecordLoadLimit, RecordTimeline, TimelineRead, TimelineReadNext, TimelineRefresh,
};
use tempfile::TempDir;

use super::*;
use crate::{
    model::{Message, Role},
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

fn raw_message(seq: u64, message: Message) -> Vec<u8> {
    let mut raw = serde_json::to_vec(&StoredMessage {
        message_ref: message_ref(seq),
        created_at: Utc::now(),
        message,
        local: LocalState::default(),
    })
    .unwrap();
    raw.push(b'\n');
    raw
}

fn append_raw(path: &Path, bytes: &[u8]) {
    let mut file = StdOpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
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
async fn routes_a_run_to_tail_first_fmtview_paging() {
    let (_workspace, store) = test_store(RunState::Open).await;
    for text in ["first", "second", "third"] {
        store
            .append_message("run-1", &Message::text(Role::User, text))
            .await
            .unwrap();
    }

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let (tail, next) = record_refs(timeline.load_older(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(tail, ["m3"]);
    assert_eq!(next, TimelineReadNext::More);
}

#[tokio::test]
async fn viewer_does_not_become_a_second_message_semantics_validator() {
    let (_workspace, store) = test_store(RunState::Open).await;
    let paths = store.paths("run-1");
    append_raw(
        &paths.messages,
        &raw_message(99, Message::text(Role::User, "observational")),
    );

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    let (records, _) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(8, usize::MAX))
            .unwrap(),
    );
    assert_eq!(records, ["m99"]);
}

#[tokio::test]
async fn exposes_complete_lines_and_hides_a_torn_tail() {
    let (_workspace, store) = test_store(RunState::Open).await;
    let paths = store.paths("run-1");
    store
        .append_message("run-1", &Message::text(Role::User, "first"))
        .await
        .unwrap();
    let second = raw_message(2, Message::text(Role::Assistant, "second"));
    let split = second.len() / 2;
    append_raw(&paths.messages, &second[..split]);

    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    assert_eq!(timeline.snapshot().pending_bytes, split as u64);
    let (visible, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(8, usize::MAX))
            .unwrap(),
    );
    assert_eq!(visible, ["m1"]);
    assert_eq!(next, TimelineReadNext::End);

    append_raw(&paths.messages, &second[split..]);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));
    let (newer, next) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(8, usize::MAX))
            .unwrap(),
    );
    assert_eq!(newer, ["m2"]);
    assert_eq!(next, TimelineReadNext::Pending);
}

#[tokio::test]
async fn maps_a_terminal_run_live_boundary_to_end() {
    let (_workspace, store) = test_store(RunState::Open).await;
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
        TimelineRefresh::NoChange(_)
    ));
    assert!(matches!(
        timeline.load_newer(RecordLoadLimit::new(8, 1024)).unwrap(),
        TimelineRead::End
    ));
}

#[tokio::test]
async fn terminal_transition_does_not_skip_newer_records_larger_than_one_batch() {
    let (_workspace, store) = test_store(RunState::Open).await;
    store
        .append_message("run-1", &Message::text(Role::User, "initial"))
        .await
        .unwrap();
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();

    let appended = (0..130)
        .map(|index| Message::text(Role::Assistant, format!("new-{index}")))
        .collect::<Vec<_>>();
    store.append_messages("run-1", &appended).await.unwrap();
    store
        .update_state("run-1", RunState::Completed)
        .await
        .unwrap();
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));

    let mut loaded = 0;
    loop {
        let TimelineRead::Records { records, next } = timeline
            .load_newer(RecordLoadLimit::new(64, usize::MAX))
            .unwrap()
        else {
            panic!("expected newer records");
        };
        loaded += records.len();
        match next {
            TimelineReadNext::More => assert!(matches!(
                timeline.refresh().unwrap(),
                TimelineRefresh::NoChange(_)
            )),
            TimelineReadNext::End => break,
            TimelineReadNext::Pending => panic!("terminal batch remained pending"),
        }
    }
    assert_eq!(loaded, appended.len());
}

#[tokio::test]
async fn ndjson_writer_copies_only_complete_records() {
    let (_workspace, store) = test_store(RunState::Open).await;
    let paths = store.paths("run-1");
    let first = raw_message(1, Message::text(Role::User, "first"));
    let second = raw_message(2, Message::text(Role::Assistant, "second"));
    append_raw(&paths.messages, &first);
    append_raw(&paths.messages, &second[..second.len() - 1]);

    let mut output = Vec::new();
    store
        .write_complete_ndjson("run-1", &mut output)
        .await
        .unwrap();
    assert_eq!(output, first);
}
