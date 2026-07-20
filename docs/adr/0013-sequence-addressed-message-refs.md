# ADR 0013: Use Sequence-Addressed Message Refs

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0005 (message identity)
- Refines: ADR 0012 (compaction boundaries and history retrieval)

## Context

Messages previously had opaque ULID refs plus a separate numeric sequence. A
history match exposed only the opaque ref, so the model could not compare the
age of results from separate searches without another field. The refs were
also long despite history tools always being scoped to one run.

Pending steering inputs reused the message ref as an idempotency key. That
mixed model-facing trajectory location with an unrelated recovery identity.

## Decision

- Every completed message in a run has the durable ref `m<N>`, where `N` is its
  one-based append sequence and its line number in the paired JSONL logs.
- The sidecar retains the numeric sequence and rejects a ref that does not
  equal `m<N>`.
- History tools expose this same ref without a separate sequence field. Their
  descriptions define result shape, ordering, and the meaning of `source`.
- A pending steering input keeps its ULID as optional sidecar metadata used
  only for idempotent recovery. It does not affect the message ref.
- Run record version 5 intentionally rejects older local run formats.

## Consequences

- Refs are short, directly ordered, and easy to relate to a trajectory line.
- Refs are unique only within a run, which is sufficient because every reader
  request already includes the run id.
- Compaction boundaries, events, search results, and reads use one identity.
- Steering recovery has an explicit idempotency field instead of overloading
  trajectory identity.

## Alternatives Considered

- **Return both an opaque ref and `seq`.** Rejected as redundant model-facing
  metadata.
- **Treat ULID lexical order as message order.** Rejected because it is opaque
  to the model and generation order need not equal committed append order.
- **Expose `m<N>` only as a history-tool alias.** Rejected because two refs for
  one message recreate a storage/display gap.

## Related Documents

- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [ADR 0005: Chat-compatible message log](0005-openai-chat-compatible-message-log.md)
- [ADR 0012: Record compaction as messages](0012-record-compaction-as-messages.md)
