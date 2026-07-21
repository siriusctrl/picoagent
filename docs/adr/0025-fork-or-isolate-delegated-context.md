# ADR 0025: Fork or Isolate Delegated Context

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0006 (durable GeneralTask child recovery)
- Refines: ADR 0017 (delegate start contract)

## Context

The original `delegate` operation always started an isolated child. That is
efficient for a self-contained task but forces the parent to restate context
for review, continuation, and parallel investigation. Copying the assistant
message containing the delegate call would be self-referential, while pointing
a child at mutable parent files would make its resume dependent on another run.

Provider prefix caching is useful when a child continues the parent's exact
request prefix, but cache behavior is provider-owned. The harness must preserve
the prefix and record reported usage without claiming a hit.

## Decision

- Require every `delegate` call to select `context: "fresh" | "fork"`.
- Fresh starts the existing isolated GeneralTask context: its own runtime
  reminder followed by the complete delegated prompt.
- Fork identifies the latest durable assistant message containing that
  provider tool-call id and freezes the preceding one-based message sequence.
  The assistant delegate turn and all of its tool results are excluded.
- Calls in one assistant tool-call batch resolve against that same latest
  assistant message, so sibling fork boundaries are identical.
- Materialize every durable parent record through the boundary in the child
  message log before appending the child reminder and prompt. Preserve message
  timestamps and compaction request/state metadata. Clear pending-input ids:
  they are run-local steering idempotency keys and are not model context.
- Preserve inherited model-facing messages and artifact refs byte-for-byte,
  including original model-visible paths. Copy each referenced artifact into
  a deterministic child-local snapshot before its message commits. History
  and ordinary reads resolve the old path to that integrity-checked local copy;
  nested forks copy from the immediate parent's snapshot.
- Build the child's active context with the ordinary compaction projection.
  Its first model request reuses the parent's exact active messages and adds
  only the child suffix; automatic compaction is deferred until after that
  first request.
- Persist context mode and fork boundary in both task coordination and child
  run records. A complete child snapshot resumes without reading parent
  trajectory files. If copying stopped before the boundary, recovery validates
  the local prefix and completes only the missing suffix from the frozen parent
  boundary.
- Fork inherits the parent's selected model. Fresh continues to use the
  configured GeneralTask model. Both use the same provider and stable system
  prompt. Full upstream cache reuse additionally requires identical frozen tool
  schemas; provider-reported `cached_input_tokens` remains the only cache
  observation.
- Advance the pre-release run and task record versions to 7. No compatibility
  layer is added for unused older runs.

## Consequences

Forked children can continue rich parent context, search the same compacted
history, and resume as portable run directories. Parallel siblings share the
longest possible parent message prefix. Fresh remains available when copying a
long context would cost more than restating the task.

Each fork child duplicates its inherited trajectory and referenced artifact
bytes on disk. This is an intentional simplicity tradeoff: local immutable
JSONL and artifacts are easier to inspect, move, and recover than cross-run
references. A process loss during the copy still needs the parent until that
fixed prefix finishes, but a completed snapshot has no such dependency.

The harness cannot guarantee a provider cache hit. Different tool schemas,
expired provider cache entries, or provider routing can report zero cached
tokens even when message prefixes match.

## Alternatives Considered

- **Always fork.** Rejected because independent tasks should not pay for a long
  irrelevant context.
- **Always stay fresh.** Rejected because callers must restate context and lose
  the opportunity to reuse the parent prefix.
- **Include the delegate assistant turn.** Rejected because the child would see
  the call that created itself without its corresponding parent result.
- **Reference parent messages lazily.** Rejected because the child would no
  longer be a self-contained resumable run.
- **Copy only active messages.** Rejected because compacted-history search and
  compaction boundaries would diverge from the parent trajectory.
- **Copy pending-input ids.** Rejected because a parent id could suppress an
  unrelated child steering message.
- **Expose cache controls in the core API.** Rejected because cache storage,
  lifetime, and routing belong to provider implementations.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [ADR 0006](0006-complete-message-resume-and-durable-child-coordination.md)
- [ADR 0017](0017-concurrent-tool-batches-and-explicit-task-controls.md)
- [ADR 0012](0012-record-compaction-as-messages.md)
