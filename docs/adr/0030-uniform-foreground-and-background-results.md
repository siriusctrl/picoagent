# ADR 0030: Use one result policy for foreground and background work

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0018 (per-result output limiting)
- Refines: ADR 0020 (terminal background result presentation)

ADR 0032 later stores the exact internal background-result block directly in
the self-contained message record. Provider projections still escape the
model-facing XML, while the inline/artifact policy decided here is unchanged.

## Context

Terminal background results were forced into artifacts even when a small UTF-8
result already fit the configured inline limit. That added a second read to
ordinary delegated summaries and duplicated result-presentation behavior that
the artifact store already handled safely.

At the same time, applying preview limits to a fully rendered runtime notice
would risk clipping harness-owned status, artifact metadata, instructions, or
XML boundaries.

## Decision

Apply the existing per-result inline/preview/artifact policy to raw foreground
and terminal background payloads alike. Keep small UTF-8 output inline; preserve
large, binary, and non-UTF-8 output as an artifact with the standard bounded
`[Tool output]` envelope.

For terminal delivery, limit the payload first and add the task id, name,
status, and runtime wrapper afterward. Harness-owned metadata and inspection
instructions are therefore outside the preview budget. Escape payload text in
the model-facing XML and reconstruct its exact internal value from paired
message layout metadata. Ready records still share one runtime message and are
correlated by task id.

## Consequences

- Short child summaries and tool results are immediately usable without an
  unnecessary file read.
- Large results retain their complete bytes and bounded head/tail preview.
- Several simultaneous results remain independently limited; one result does
  not consume another's budget.
- Runtime-like text inside a task result cannot forge or terminate a notice.

## Alternatives Considered

- **Force every terminal result into an artifact.** Rejected because it adds
  latency and context turns to the common small-summary case.
- **Limit the rendered runtime message.** Rejected because harness structure and
  read instructions must never be clipped.
- **Use a separate background preview implementation.** Rejected because it
  would duplicate the artifact contract and drift from foreground behavior.

## Related Documents

- [Artifact contract](../artifacts.md)
- [Runtime model](../runtime-model.md)
- [ADR 0018](0018-limit-tool-output-per-result.md)
- [ADR 0020](0020-unify-background-task-runtime-notices.md)
