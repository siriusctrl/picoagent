# ADR 0041: Close Active Agent Threads

- Status: Accepted
- Date: 2026-07-23
- Refines: ADR 0038 (runtime handle close semantics)

## Context

Requiring `stop` before `close` exposed an unnecessary two-step sequence to the
model. Closing is already the permanent lifetime operation, so a caller asking
to close a running thread has also expressed that its current activity should
not continue.

The durable authority remains the child run's open or closed state. Generic
tool start/completion events describe the attempted action, while the existing
agent-closed event describes the completed domain transition; none of those
events participate in recovery.

## Decision

- Let `close` operate on an idle or active agent thread.
- Linearize close in memory before waiting: mark the handle closed and reject
  new input as soon as the operation owns the close.
- Signal the exact active generation, abort its tracked execution, and wait for
  cleanup when one exists.
- Clear pending input and queued followups, then persist the child run as
  `Closed` and emit the existing agent-closed event.
- Add no closing state, cancellation result, recovery record, or close-specific
  start/end events.

## Consequences

- One call is sufficient to end an agent lifetime regardless of activity state.
- Concurrent sends lose once close has linearized, so no accepted input can be
  stranded behind a closed thread.
- A process crash before the durable state update leaves the thread open; a
  crash after it leaves the thread closed. Restart does not reconstruct the
  interrupted close attempt from events.
- `stop` remains useful when the caller wants to preserve the thread for later
  input.

## Alternatives Considered

- **Require stop then close.** Rejected because it makes a permanent operation
  depend on a separate temporary one.
- **Persist a closing state.** Rejected because transparent close recovery is
  not a required guarantee and would add another crash state machine.
- **Emit dedicated close start/end records.** Rejected because generic tool
  lifecycle events plus the durable run state already express the useful facts.

## Related Documents

- [ADR 0038](0038-runtime-handles-and-explicit-restart.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
