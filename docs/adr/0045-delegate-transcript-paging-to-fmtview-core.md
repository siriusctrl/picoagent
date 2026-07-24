# ADR 0045: Delegate Transcript Paging to fmtview-core

- Status: Accepted
- Date: 2026-07-24
- Refines: ADR 0044 (inspection implementation)

## Context

After newline-complete messages replaced logical checkpoints, Fiasco still
owned about one thousand lines of transcript-viewer code for reverse newline
scans, bounded forward batches, torn-tail tracking, replacement samples,
short-read retries, reset epochs, and instrumentation. This was generic
growing-JSONL behavior rather than agent-runtime behavior.

fmtview 0.6.2 already implements the same physical-file concerns in its public
`fmtview_core::FileRecordTimeline`. The higher-level `fmtview::view` embedding
facade exports the `RecordTimeline` contract but does not export that concrete
source.

Inspection is observational. Strict message refs, provider conversation shape,
and trailing tool-call/result repair matter before the next writer resumes a
run, but do not require a second validation and recovery authority in the
interactive viewer.

## Decision

- Depend directly on exactly `fmtview-core` 0.6.2 alongside fmtview 0.6.2 and
  use its public `FileRecordTimeline` as the interactive physical-file source.
- Keep a small Fiasco `TranscriptTimeline` adapter for run-directory routing,
  run-record validation at open/refresh, and mapping terminal run states to the
  generic timeline's `End` boundary.
- Keep redirected NDJSON on Fiasco's existing complete-line reader so it
  preserves exact bytes and does not construct the TUI timeline.
- Do not parse Fiasco messages, validate sequence refs, or implement additional
  truncate/rewrite recovery in the inspector. Writer/resume trajectory loading
  remains the semantic authority. If an unusual concurrent rewrite cannot be
  followed, the operator can reopen the viewer.
- Keep terminal rendering, lazy formatted/raw spools, search, navigation,
  follow state, and event handling in fmtview.

## Consequences

- Fiasco deletes its reverse scan, refresh tracker, sampling, retry, cursor, and
  instrumentation implementations and their duplicate tests.
- Tail-first opening, bounded paging, torn-line hiding, and ordinary growing
  file refresh retain fmtview's tested behavior.
- Interactive inspection no longer rejects an old malformed complete message
  as it is paged. Resume and full trajectory loading still reject malformed
  durable conversations before another model request.
- The direct fmtview-core dependency exposes a lower-level library boundary,
  but it is smaller than maintaining a local implementation. Pinning the same
  release as fmtview ensures both use one `RecordTimeline` type.

## Alternatives Considered

- **Keep the Fiasco-specific timeline.** Rejected because newline paging and
  physical refresh are not orchestration semantics and duplicate fmtview.
- **Wait for the fmtview facade to re-export `FileRecordTimeline`.** Rejected
  because it preserves the duplicate implementation until another dependency
  release for no product benefit.
- **Wrap fmtview-core with Fiasco record validation.** Rejected because it would
  recreate semantic cursor state in an observational reader.
- **Load the complete transcript before opening.** Rejected because startup and
  memory would scale with total history and lose tail-first inspection.

## Related Documents

- [Architecture](../architecture.md)
- [Entrypoints](../entrypoints.md)
- [Source map](../source-map.md)
- [ADR 0037](0037-embed-fmtview-over-checkpoint-timeline.md)
- [ADR 0044](0044-newline-visible-messages-and-tail-repair.md)
