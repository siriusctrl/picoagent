# ADR 0001: Persist Complete Messages and Keep Stream Deltas Transient

- Status: Accepted
- Date: 2026-07-14

ADR 0005 refines only the physical encoding of these completed messages:
`messages.jsonl` now contains Chat-compatible lines, while stable identity and
provider-neutral reconstruction state live in `message_metadata.jsonl`. The
completed-message boundary and transient-delta decisions below remain accepted.

## Context

Picoagent needs a portable, searchable trajectory for inspection and future
resume behavior. Provider streams can produce hundreds of small text and
reasoning chunks. Persisting each chunk in `events.jsonl` duplicates content
already assembled in `messages.jsonl`, adds repeated event metadata, and records
partial output that cannot be a safe resume boundary.

At the same time, interactive and machine-readable clients still benefit from
receiving deltas as they arrive. Lifecycle, usage, tool, artifact, and failure
events remain useful for operational inspection and debugging.

## Decision

- A completed message is the durable trajectory and resumable boundary.
- `messages.jsonl` stores complete user, assistant, tool, and explicit reasoning
  content in conversation order.
- `ModelDelta` and `ModelReasoningDelta` remain runtime events for live sinks,
  including `pico run --output ndjson`.
- `RunDirStore` does not write either per-chunk delta to `events.jsonl`.
- `events.jsonl` retains run/model lifecycle, usage, tool, artifact, background
  task, subagent, and failure events.
- A failed provider stream does not create a partial assistant message. Retry
  policy is separate from this persistence decision.

## Consequences

- Agents and developers can use `rg`, `jq`, or ordinary file reads over
  `messages.jsonl` without reconstructing a stream.
- Run directories are substantially smaller for long or high-reasoning calls.
- Live consumers still receive low-latency deltas, but those deltas cannot be
  recovered from a completed run directory.
- Debugging exact provider chunk boundaries requires live capture or a focused
  provider test rather than the durable run log.

## Alternatives Considered

- **Persist every chunk.** Rejected because it duplicates the transcript and
  makes event volume proportional to provider chunking behavior.
- **Coalesce chunks before persistence.** Rejected because the resulting
  boundaries are arbitrary and still duplicate complete messages.
- **Persist only `final.md`.** Rejected because it loses tool interactions,
  intermediate assistant messages, and explicit reasoning content needed for a
  useful trajectory.

## Related Documents

- [ADR 0005: Chat-compatible message log](0005-openai-chat-compatible-message-log.md)
- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
