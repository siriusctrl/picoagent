# ADR 0012: Record Compaction as Messages

- Status: Accepted
- Date: 2026-07-20
- Supersedes: ADR 0003, ADR 0007
- Refines: ADR 0004 (compaction request profile)

## Context

The original local compaction design flattened an older native message prefix
into Markdown, sent it through a separate tool-free system profile, stored the
result in `compactions.jsonl`, and later wrapped it in a synthetic user
`<compacted-history>` message. That created three representations of the same
state and a visible gap between the durable Chat trajectory, history tools, and
the context sent to the model.

Picoagent is an internal harness with no deployed run-format compatibility
requirement. A smaller persistence model and directly inspectable trajectories
are more valuable than preserving the separate checkpoint format.

## Decision

- A compaction request uses the run's existing provider, model, invariant system
  prompt, and frozen tool schemas. Its message input is the initial user message,
  the previous assistant compacted state when one exists, native ordinary
  messages being replaced, and one final user instruction from
  `prompts/agents.yaml`.
- Tool calls returned from this request are rejected and never executed. A
  failed compaction leaves the previous/full active context in use.
- After a successful response, both the compaction user instruction and exact
  assistant response are appended to `messages.jsonl` as ordinary
  OpenAI-Chat-compatible messages. No `compactions.jsonl` exists.
- `message_metadata.jsonl` classifies compaction request/state records. The
  assistant state's metadata records only the covered-through and first-kept
  refs. Provider/model already live in `run.json`, usage lives in events, and
  message order identifies the latest state. The assistant metadata line is the
  effective compaction commit marker.
- Normal request assembly is a projection, not a literal replay of log order. It
  sends the initial user message, the latest exact assistant compacted state,
  and exact ordinary messages beginning at the first-kept ref. Compaction
  instructions and older compaction states remain durable but are not replayed.
- A trailing compaction request without an assistant state is inert. Resume
  excludes it from normal context and may retry compaction later.
- History retrieval searches and reads omitted ordinary messages. Compaction
  control/state records remain directly inspectable in the run files but are
  not treated as recovered raw evidence.
- `compact_at_tokens` is the soft tracked-input trigger.
  `context_window_tokens`, when configured, is the model's larger nominal full
  window. A provider-neutral request estimate is checked from the first call
  and refined by provider usage when available; it is not tokenizer-exact. The
  Root profile must configure `runtime.max_output_tokens` so its output reserve
  is explicit.
  `keep_recent_tokens` selects the exact suffix and
  `summary_max_output_tokens` bounds state generation.

## Consequences

- The Chat log shows the compaction interaction exactly as sent and returned,
  while private boundaries remain outside the OpenAI-compatible message shape.
- Resume, history retrieval, and active-context assembly derive state from one
  paired message log instead of coordinating a second JSONL file.
- The compaction request benefits from the same stable prompt/tool prefix as
  normal calls. Schemas are present but cannot cause side effects because tool
  responses fail the compaction attempt.
- Durable order and provider-input order intentionally differ after compaction:
  the state is appended after messages it summarizes, then projected before the
  exact recent suffix on later requests.
- Run record version 4 intentionally rejects older local run formats.

## Alternatives Considered

- **Keep separate append-only checkpoints.** Rejected because it preserves a
  parallel persistence mechanism and requires summary wrapping on replay.
- **Persist only the assistant summary.** Rejected because it hides the user
  instruction that produced the state and makes trajectory auditing incomplete.
- **Replay the compaction instruction on every later request.** Rejected because
  it is a one-time control command, not continuing task context.
- **Flatten messages before summarization.** Rejected because it discards native
  role, tool-call/result, reasoning, and provider-item structure.
- **Use only provider-side compaction.** Rejected because it is not portable
  across the supported adapters and may produce opaque state.

## Related Documents

- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Configuration](../configuration.md)
- [ADR 0005: Chat-compatible message log](0005-openai-chat-compatible-message-log.md)
