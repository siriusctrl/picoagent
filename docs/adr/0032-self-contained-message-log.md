# ADR 0032: Store Each Message as One Self-Contained Record

- Status: Accepted
- Date: 2026-07-21
- Supersedes: ADR 0005
- Refines: ADR 0001 (completed-message encoding and commit boundary)
- Refines: ADR 0013 (sequence-addressed refs)
- Refines: ADR 0030 (background result persistence)

## Context

ADR 0005 optimized the transcript for direct OpenAI Chat familiarity. The
runtime message and the local state needed to reconstruct it could not fit that
wire shape, so every durable message became a coordinated pair across
`messages.jsonl` and `message_metadata.jsonl`. Correctness then required two
hashes, byte-span content layouts, two-file recovery, a per-run message-log
lock, and cursors validated against both file lengths.

Picoagent is an internal harness without released-run compatibility
requirements. A run has one process that interacts with it and may have many
read-only viewers. The provider-neutral `Message` is already small, typed, and
understandable when serialized. Provider adapters already own their wire
projections, so the durable format does not need to imitate one provider.

## Decision

- `run.json` identifies the durable format as `pico-message` and uses run record
  version 9. Older development runs are intentionally not decoded.
- Each `messages.jsonl` line is one self-contained record with:
  - `ref`, equal to `m<N>` for its one-based line number;
  - `created_at`;
  - `role` and the exact provider-neutral typed `content` blocks;
  - optional `_pico` state for pending-input idempotency and compaction
    classification/boundaries.
- Sequence is derived from `ref` and line position rather than duplicated.
  Tool-error state, artifact refs, provider continuation items, background-task
  identity, reasoning, and images remain in their ordinary content blocks.
- `message_metadata.jsonl`, content-layout reconstruction, message hashes, the
  reconstruction hash, and the message-log lock are removed.
- The process holding the run execution lease is the sole writer. Multiple
  independent writers for one run are unsupported. Read-only viewers do not
  acquire the execution lease or a message-log lock.
- A newline is the commit marker. Viewers expose only newline-terminated
  records, ignoring a partial final line. Before appending after an interruption,
  the writer validates the committed prefix and truncates the uncommitted tail.
  Malformed committed records and out-of-sequence refs remain errors.
- Provider requests and model-facing `history_read` output continue to use
  adapter-owned projections. Persisted messages do not claim wire compatibility
  with OpenAI Chat, Responses, or Anthropic.

## Consequences

- One file and one line are the complete source of truth for a message. Direct
  inspection shows the same semantic blocks the runner will replay.
- Resume, compaction, history, fork snapshots, artifact lookup, reasoning, and
  background delivery no longer depend on sidecar joins or byte spans.
- Viewers can safely tail or scan a live run without coordinating with the
  writer. They may briefly omit the currently appending record.
- The store detects malformed committed JSON and ordering mistakes but does not
  attempt to detect deliberate same-shape mutation with hashes. Run directories
  are trusted local state; artifact bytes retain their separate integrity
  contract.

- The runtime must enforce the sole-writer assumption through the execution
  lease. Code that bypasses the runner and concurrently appends through
  independent stores violates the contract.

ADR 0033 later removes fork snapshots; the one-record message contract remains
unchanged.

ADR 0034 later groups those one-record lines into atomic logical checkpoints.
A newline still completes one physical record, while readers publish a group
only after every declared line is present.

## Alternatives Considered

- **Put the old sidecar under `_pico` unchanged.** Rejected because one file
  would remove the commit join but retain hashes, spans, and reconstruction
  complexity without a real boundary requiring them.
- **Keep OpenAI Chat fields and store only exceptional internal blocks under
  `_pico`.** Rejected because reconstruction would still need ordering rules and
  provider-specific exceptions for images, opaque items, and background tasks.
- **Persist both provider-neutral and Chat projections.** Rejected because it
  creates duplicate message bodies and ambiguity over which copy is canonical.
- **Support several writers with file locking.** Rejected because the product
  permits only one interactor per run. The run execution lease already expresses
  that ownership; extra message-level coordination serves no valid workflow.
- **Add a migration path for earlier runs.** Rejected because no external or
  released run depends on the pre-release format.

## Related Documents

- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Design choices](../design-choices.md)
- [ADR 0001: Durable messages and transient deltas](0001-durable-messages-transient-stream-deltas.md)
- [ADR 0005: Chat-compatible messages with a metadata sidecar](0005-openai-chat-compatible-message-log.md)
- [ADR 0013: Sequence-addressed message refs](0013-sequence-addressed-message-refs.md)
