# ADR 0018: Limit Tool Output Per Result

- Status: Accepted
- Date: 2026-07-20

## Context

Picoagent originally combined a per-result inline threshold with a cumulative
preview budget for the complete run. After enough ordinary tool output, even a
later small result became a reference-only artifact. Compaction did not reset
the budget, so a run with newly available context still hid later output. The
runtime then needed exceptions for reads, history, task controls, confirmations,
and errors, making a simple output policy hard to explain and maintain.

## Decision

- Limit every tool result independently.
- Keep a small UTF-8 result inline when it fits `artifacts.inline_bytes`.
- Spill a larger or non-text result in full and return one bounded head/tail
  preview plus immutable artifact metadata.
- Do not keep, persist, restore, or configure a cumulative run or turn preview
  budget.
- Compaction controls conversation context; artifact policy controls the size
  of each result. Neither changes the other's limits.

## Consequences

- A previous large result never suppresses a later small confirmation, error,
  task status, or command output.
- Resume no longer reconstructs preview-budget state, and background tasks do
  not reserve preview bytes before delivery.
- A model can still issue many individually bounded outputs in one run. The
  normal context-window check and compaction handle that aggregate growth.
- The artifact policy has one fewer configuration field and no tool-specific
  escape-hatch marker.

## Alternatives Considered

- **Keep the cumulative run budget and reset it after compaction.** Rejected
  because it couples artifact presentation to compaction timing and still needs
  recovery accounting.
- **Share a budget across one assistant tool batch.** Rejected for now because
  concurrent and promoted background results make ownership and deterministic
  allocation more complex than the demonstrated need.
- **Destructively truncate large output.** Rejected because complete artifacts
  are useful for exact inspection and trajectory search.

## Related Documents

- [Artifacts](../artifacts.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
