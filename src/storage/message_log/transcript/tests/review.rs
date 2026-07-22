use super::*;

#[tokio::test]
async fn concurrent_pending_truncate_retries_without_polluting_suffix_state() {
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
            Message::text(Role::Assistant, format!("discarded-{}", "x".repeat(4096))),
            Message::text(Role::User, "missing"),
        ],
    );
    let replacement = raw_checkpoint(2, vec![Message::text(Role::Assistant, "replacement")]);
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    append_raw(&paths.messages, &discarded[..1]);

    let mut attempts = Vec::new();
    let refresh = timeline
        .refresh_timeline_with_hook(|attempt| {
            attempts.push(attempt);
            if attempt == 1 {
                StdOpenOptions::new()
                    .write(true)
                    .open(&paths.messages)
                    .unwrap()
                    .set_len(committed_end)
                    .unwrap();
                append_raw(&paths.messages, &replacement);
            }
            Ok(())
        })
        .unwrap();
    assert!(matches!(refresh, TimelineRefresh::Appended(_)));
    assert_eq!(attempts, [1, 2]);
    assert_eq!(timeline.snapshot().epoch, 1);

    let (records, next) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
    );
    assert_eq!(records, ["m2"]);
    assert_eq!(next, TimelineReadNext::Pending);
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::NoChange(_)
    ));
    assert!(matches!(
        timeline
            .load_newer(RecordLoadLimit::new(128, 1024 * 1024))
            .unwrap(),
        TimelineRead::Pending
    ));
}

#[tokio::test]
async fn forward_budget_stops_before_a_huge_checkpoint_after_a_small_append() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "initial"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    append_raw(
        &paths.messages,
        &raw_checkpoint(2, vec![Message::text(Role::Assistant, "small append")]),
    );
    append_raw(
        &paths.messages,
        &raw_checkpoint(
            3,
            (0..256)
                .map(|index| {
                    Message::text(Role::User, format!("large-{index}-{}", "x".repeat(4096)))
                })
                .collect(),
        ),
    );
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));

    let before = timeline.instrumentation();
    let (small, next) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(2, 1024 * 1024))
            .unwrap(),
    );
    let after_small = timeline.instrumentation();
    assert_eq!(small, ["m2"]);
    assert_eq!(next, TimelineReadNext::More);
    assert!(after_small.bytes_read - before.bytes_read < 64 * 1024);

    let (large, next) = record_refs(timeline.load_newer(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(large.len(), 256);
    assert_eq!(large.first().unwrap(), "m3");
    assert_eq!(large.last().unwrap(), "m258");
    assert_eq!(next, TimelineReadNext::Pending);
}

#[tokio::test]
async fn forward_byte_preflight_restores_the_buffered_reader_position() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "initial"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    append_raw(
        &paths.messages,
        &raw_checkpoint(2, vec![Message::text(Role::Assistant, "small append")]),
    );
    append_raw(
        &paths.messages,
        &raw_checkpoint(
            3,
            vec![
                Message::text(Role::Assistant, "first in group"),
                Message::text(Role::User, "x".repeat(32 * 1024)),
            ],
        ),
    );
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));

    let (records, next) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(4, 1024 * 1024))
            .unwrap(),
    );
    assert_eq!(records, ["m2", "m3", "m4"]);
    assert_eq!(next, TimelineReadNext::Pending);
}

#[tokio::test]
async fn forward_byte_budget_scans_only_the_bounded_prefix_of_the_next_checkpoint() {
    let (_workspace, store) = test_store(RunState::Running).await;
    store
        .append_message("run-1", &Message::text(Role::User, "initial"))
        .await
        .unwrap();
    let paths = store.paths("run-1");
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();
    append_raw(
        &paths.messages,
        &raw_checkpoint(2, vec![Message::text(Role::Assistant, "small append")]),
    );
    append_raw(
        &paths.messages,
        &raw_checkpoint(
            3,
            vec![
                Message::text(Role::Assistant, "small first line"),
                Message::text(Role::User, "x".repeat(1024 * 1024)),
            ],
        ),
    );
    assert!(matches!(
        timeline.refresh().unwrap(),
        TimelineRefresh::Appended(_)
    ));

    let before = timeline.instrumentation();
    let (small, next) = record_refs(
        timeline
            .load_newer(RecordLoadLimit::new(10, 8 * 1024))
            .unwrap(),
    );
    let after_small = timeline.instrumentation();
    assert_eq!(small, ["m2"]);
    assert_eq!(next, TimelineReadNext::More);
    assert!(after_small.bytes_read - before.bytes_read < 32 * 1024);

    let (large, next) = record_refs(timeline.load_newer(RecordLoadLimit::new(1, 1)).unwrap());
    assert_eq!(large, ["m3", "m4"]);
    assert_eq!(next, TimelineReadNext::Pending);
}

#[tokio::test]
async fn forward_preflight_rejects_malformed_checkpoint_start_before_budget_skip() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    append_raw(
        &paths.messages,
        &raw_checkpoint(1, vec![Message::text(Role::Assistant, "valid prefix")]),
    );
    let mut malformed = raw_checkpoint(2, vec![Message::text(Role::User, "malformed")]);
    let mut value: serde_json::Value = serde_json::from_slice(&malformed[0]).unwrap();
    value["_fiasco"]["checkpoint"]["index"] = serde_json::json!(1_u64);
    value["_fiasco"]["checkpoint"]["count"] = serde_json::json!(1_000_000_u64);
    malformed[0] = serde_json::to_vec(&value).unwrap();
    malformed[0].push(b'\n');
    append_raw(&paths.messages, &malformed);
    append_raw(
        &paths.messages,
        &raw_checkpoint(3, vec![Message::text(Role::Assistant, "valid tail")]),
    );
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();

    let error = timeline
        .probe_prefix(RecordLoadLimit::new(2, 1024 * 1024))
        .unwrap_err();
    assert!(error.to_string().contains("starts at index 1 instead of 0"));
}

#[tokio::test]
async fn skipped_malformed_older_checkpoint_makes_progress_then_errors() {
    let (_workspace, store) = test_store(RunState::Running).await;
    let paths = store.paths("run-1");
    let mut malformed = raw_checkpoint(1, vec![Message::text(Role::User, "malformed")]);
    let mut value: serde_json::Value = serde_json::from_slice(&malformed[0]).unwrap();
    value["_fiasco"]["checkpoint"]["count"] = serde_json::json!(1_000_000_u64);
    malformed[0] = serde_json::to_vec(&value).unwrap();
    malformed[0].push(b'\n');
    append_raw(&paths.messages, &malformed);
    append_raw(
        &paths.messages,
        &raw_checkpoint(2, vec![Message::text(Role::Assistant, "tail")]),
    );
    let mut timeline = TranscriptTimeline::open(&store, "run-1").unwrap();

    let (tail, next) = record_refs(
        timeline
            .load_older(RecordLoadLimit::new(2, 1024 * 1024))
            .unwrap(),
    );
    assert_eq!(tail, ["m2"]);
    assert_eq!(next, TimelineReadNext::More);
    let error = timeline
        .load_older(RecordLoadLimit::new(2, 1024 * 1024))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("incomplete checkpoint ends inside committed transcript")
    );
}
