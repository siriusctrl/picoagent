# ADR 0021: Mark an Active Compacted State as Continuation Context

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0012 (normal active-context projection)

## Context

ADR 0012 projects the exact assistant compacted state before the exact recent
suffix and omits the one-time compaction request. In a live skills task, a model
saw the assistant state and returned another `# Compacted state` as its final
answer instead of continuing the pending work. The durable messages were
correct, but the normal projection did not explain the reordered boundary.

## Decision

- After an active assistant compacted state, insert one synthetic user
  `<runtime-reminder>` before the exact recent suffix.
- The stable reminder prose lives in `prompts/agents.yaml`. It says the state is
  context, not a final answer or another compaction request, and tells the model
  to continue the original task.
- Do not append this reminder to `messages.jsonl`. Do not emit it for a run with
  no completed compacted state or in the compaction request itself.

## Consequences

- Normal requests make the projection boundary explicit without changing the
  system prompt, frozen tool schemas, or exact stored assistant state.
- The provider-visible projection contains one small message not present in the
  durable Chat trajectory; its fixed content is directly defined in the prompt
  registry and its placement is covered by tests.

## Alternatives Considered

- **Replay the compaction request.** Rejected because it asks the model to
  compact again and ADR 0012 intentionally treats it as one-time control.
- **Rewrite the assistant state as a user message.** Rejected because it loses
  exact role-preserving reuse of the durable state.
- **Detect and retry final answers beginning with `# Compacted state`.** Rejected
  because it adds output-pattern policy instead of clarifying the ambiguous
  input boundary.

## Related Documents

- [ADR 0012: Record compaction as messages](0012-record-compaction-as-messages.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
