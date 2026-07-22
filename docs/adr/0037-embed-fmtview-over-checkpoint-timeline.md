# ADR 0037: Embed fmtview Over a Checkpoint Timeline

- Status: Accepted
- Date: 2026-07-22
- Refines: ADR 0032 (read-only message-log inspection)
- Refines: ADR 0034 (checkpoint visibility boundary)

## Context

Operators need to inspect both completed and actively growing run transcripts.
Large histories must open at the useful tail without indexing or formatting the
whole file. A live viewer must not expose a torn physical line or any member of
an incomplete logical checkpoint, and writer recovery must not have a second,
slightly different definition of committed messages.

fmtview already owns lazy formatting, search, structured navigation, wrapping,
follow attachment state, and terminal lifecycle. Copying those behaviors into
Fiasco would couple the orchestration runtime to ratatui/crossterm details.
Teaching fmtview about Fiasco run directories or checkpoint metadata would move
the storage boundary in the other direction.

## Decision

- Fiasco implements fmtview's backend-neutral `RecordTimeline` for one
  `messages.jsonl`. It owns run lookup, run-state liveness, checkpoint-aware
  loading, raw bytes, physical offsets, and reset epochs.
- fmtview owns all formatting, raw/formatted spools, viewport, search,
  navigation, follow state, rendering, input polling, and terminal cleanup.
  Fiasco depends on the released `fmtview` crate only, not `fmtview-core` and
  not a path or Git revision.
- The source opens at the committed tail. It discovers the last complete group
  by reverse physical-line scanning, then feeds that candidate forward through
  the same `CheckpointDecoder` used by trajectory loading and append recovery.
  Older reads repeat this by whole groups; newer reads continue forward from a
  known sequence and byte boundary. A checkpoint can cross a load budget only
  as the first group in a batch and is never partially published.
- Follow refresh retains the pending decoder, torn-line bytes, and scan cursor.
  Bounded samples detect committed-prefix and pending-suffix changes. Rewriting
  only an uncommitted tail rebuilds pending state from the same committed
  boundary without duplicating records. Refresh retries a concurrently changing
  file without publishing partial decoder progress. Committed truncation or
  replacement starts a new record-id epoch.
- Queued, running, and idle run states are live. Completed, failed, cancelled,
  and closed states are terminal.
- On a TTY, `fiasco inspect <run-id>` opens a tail-first snapshot and
  `--follow` enables refresh. Redirected stdout defaults to exact committed
  NDJSON; `--output ndjson` selects it explicitly. `--summary` keeps the former
  metadata and final-output view. Redirected follow is rejected.
- Inspect dispatch occurs before application config/provider initialization.
  Transcript reads do not require provider credentials, tools, MCP servers,
  hooks, or a valid provider config.
- Events, child-run trees, and task control are not part of this source or the
  first inspector surface.

## Consequences

- Completed and growing runs use one viewer without adding terminal backend
  code to Fiasco or Fiasco storage types to fmtview.
- First screen work is proportional to the EOF checkpoint/suffix rather than
  total history. Search and older navigation may validate earlier checkpoints
  lazily as they are requested.
- NDJSON is safe for pipes and preserves provider tool-argument strings and all
  other raw record bytes exactly.
- Reverse lazy validation can discover an old malformed checkpoint only when a
  user navigates or searches into that portion. This is intentional; resume and
  whole-trajectory loading continue to validate the complete committed prefix.
- Bounded identity samples are mutation detection for the supported sole-writer
  workflow, not a cryptographic integrity scheme for hostile run directories.

## Alternatives Considered

- **Build a Fiasco-specific ratatui viewer.** Rejected because it duplicates
  fmtview's rendering, search, navigation, follow state, and terminal cleanup.
- **Teach fmtview to decode `.fiasco` runs.** Rejected because a general viewer
  should consume records, not own orchestration storage semantics.
- **Read the entire trajectory before opening.** Rejected because startup and
  memory would scale with total history and defeat tail-first follow.
- **Expose newline-complete records immediately.** Rejected because a logical
  assistant/tool turn would become visible in a partial state and disagree with
  resume.
- **Create a separate persistent transcript index.** Rejected because reverse
  checkpoint scans and lazy directional cursors meet the current access pattern
  without another durability/recovery contract.

## Related Documents

- [Architecture](../architecture.md)
- [Entrypoints](../entrypoints.md)
- [Source map](../source-map.md)
- [ADR 0032](0032-self-contained-message-log.md)
- [ADR 0034](0034-atomic-turn-checkpoints.md)
