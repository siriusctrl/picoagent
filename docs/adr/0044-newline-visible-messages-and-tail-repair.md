# ADR 0044: Expose Complete Lines and Repair Only the Resume Tail

- Status: Accepted
- Date: 2026-07-24
- Supersedes: ADR 0034 (atomic multi-message checkpoints)
- Refines: ADR 0037 (transcript visibility)
- Refines: ADR 0038 (restart boundary)
- Refines: ADR 0043 (checkpoint payload encoding)

## Context

Atomic tool-turn checkpoints made a read-only viewer wait for every result
before showing any message from the batch. Supporting that guarantee required
checkpoint metadata on each member, a group decoder shared by all readers, and
group-aware forward and reverse scans.

The guarantee is not needed. Tools finish before the runtime appends their
assistant/result batch, so partial visibility normally exists only during the
physical write. A process crash in that window is rare, and an inspector may
show the completed prefix until the next writer repairs it. Crash recovery does
still need a provider-valid conversation before making another model request.

## Decision

- A terminating newline is the only durable visibility boundary. Every complete
  message line is immediately available to trajectory readers and inspectors.
- Remove `_fiasco.checkpoint`, `index`, and `count`. Keep `_fiasco` only for
  message-local compaction classification.
- Keep one writer and append an ordered assistant/result batch with one
  `write_all`, flush, and sync sequence. This is an ordering optimization, not a
  multi-record transaction guarantee.
- Hide only a torn final physical line. A live viewer may display a prefix of
  the final tool turn and later see it completed or removed.
- Before an existing run starts another activity, scan its complete lines in
  order. Match a trailing assistant's tool calls against tool results in
  original call order, including repeated ids by occurrence. If EOF arrives
  before every result, truncate from that assistant's byte offset.
- Treat a compaction request without its following state as inert. Do not add a
  separate transaction or recovery rule for it. A missing optional attachment
  also needs no repair; the crash reminder lets the model inspect or reread.
- Keep crash handling explicit: append the restart notice and let the model
  decide what to retry. Never synthesize results or replay the discarded calls.
- Advance the pre-release run record to version 14. Earlier development runs
  are not decoded or migrated.

## Consequences

- The generic checkpoint decoder, pending-group state, reverse group discovery,
  and per-message membership metadata disappear.
- Transcript paging operates on individual physical messages. Record and byte
  limits no longer expand to fit a whole logical turn.
- A viewer can temporarily present an assistant call without all results. This
  is accepted because inspection is observational and refresh remains
  available.
- ADR 0045 delegates the viewer's generic newline paging and refresh mechanics
  to fmtview-core; strict message and trailing-turn validation remain on the
  writer/resume path.
- Resume has one semantic repair rule at the writer boundary. Complete tool
  turns, final assistants, ordinary user messages, and compacted states remain
  untouched.
- Tool call ids are recovery correlation within the trailing turn, not a new
  durable identity or scheduler authority.

## Alternatives Considered

- **Keep atomic checkpoints for viewer consistency.** Rejected because it makes
  every normal reader pay for a rare crash presentation concern.
- **Match tool calls inside the viewer.** Rejected because it moves provider
  conversation semantics into a read-only presentation path and still needs
  special handling for compaction and attachments.
- **Persist a commit marker after each batch.** Rejected because it retains a
  second record type and group protocol when temporary partial visibility is
  acceptable.
- **Keep every complete line on resume, even an unmatched tool call.** Rejected
  because provider APIs require tool calls and results to form a valid
  conversation.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
- [ADR 0032](0032-self-contained-message-log.md)
- [ADR 0034](0034-atomic-turn-checkpoints.md)
- [ADR 0037](0037-embed-fmtview-over-checkpoint-timeline.md)
- [ADR 0038](0038-runtime-handles-and-explicit-restart.md)
- [ADR 0043](0043-compact-message-and-checkpoint-payloads.md)
- [ADR 0045](0045-delegate-transcript-paging-to-fmtview-core.md)
