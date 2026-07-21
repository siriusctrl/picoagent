# ADR 0029: Recover incomplete model output at narrow boundaries

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0001 (completed-message persistence boundary)
- Refines: ADR 0017 (independent calls in one tool batch)

## Context

OpenAI-compatible function arguments arrive as JSON-encoded strings. Parsing
one string while assembling the provider response made one malformed call abort
the whole assistant message and discard valid sibling calls. Separately, an
output-token stop or a missing protocol terminal event can end an otherwise
healthy request before it produces a complete assistant message.

These failures need bounded recovery without turning transport errors into
blind retries or persisting partial conversation state.

## Decision

Keep every provider function-call argument string exact in the internal message
and Chat-compatible log. Parse it only when that individual tool executes. A
malformed nonempty value becomes that call's normal ordered error result; valid
siblings still execute. Resume can reconstruct the same deterministic error
without running the tool.

Classify only explicit output-limit stops and streams missing their required
terminal event as structurally incomplete. Discard their partial assistant
content and issue at most one fresh normal request. The repair request reuses
the stable system prompt, frozen tool schemas, and existing messages, then adds
one non-durable runtime reminder at the tail asking for a complete replacement.
Each real request emits its own lifecycle events. Transport, authentication,
filtering/refusal, malformed SSE, deadline, and other provider errors are not
repaired this way. A discarded attempt retains any provider-reported input,
output, cached-input, and reasoning usage in its failure event.

## Consequences

- One bad tool argument no longer loses valid calls or their original ordering.
- The next model turn sees the exact malformed call and its correlated error and
  can correct it normally.
- A repair cannot loop indefinitely, and discarded partial text never becomes a
  resumable message or executes a partial tool call.
- Anthropic replay parses valid stored arguments into native `tool_use.input`;
  malformed stored arguments use `{}` beside their matching error result because
  that wire format cannot represent malformed JSON.

## Alternatives Considered

- **Parse every call during provider assembly.** Rejected because the failure
  scope becomes the whole assistant response.
- **Retry every model error.** Rejected because side effects, authentication,
  deadlines, and invalid protocol data need explicit handling rather than an
  implicit duplicate request.
- **Persist partial assistant text before repair.** Rejected because completed
  messages are the resumable boundary.

## Related Documents

- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [ADR 0001](0001-durable-messages-transient-stream-deltas.md)
- [ADR 0017](0017-concurrent-tool-batches-and-explicit-task-controls.md)
