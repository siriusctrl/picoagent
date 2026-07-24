# ADR 0042: Store Compatible Chat Reasoning Beside Assistant Content

- Status: Accepted
- Date: 2026-07-24
- Refines: ADR 0001 (completed reasoning persistence)
- Refines: ADR 0032 (provider-neutral message shape)

ADR 0043 later removes the redundant provider label from opaque continuation
items and advances the pre-release run record to version 13. The separate Chat
reasoning field remains unchanged.

## Context

Compatible Chat endpoints return assistant `reasoning_content`, visible
`content`, and `tool_calls` as sibling fields. Fiasco instead represented Chat
reasoning as a typed content block. Replaying it required scanning all content
blocks and joining them with an invented newline, so the provider-neutral
representation could not guarantee the exact text required by keep-thinking
continuations.

OpenAI Responses reasoning has a different contract: encrypted reasoning is an
ordered provider output item alongside text and function calls. Treating both
protocols as one generic reasoning block would erase that distinction.

## Decision

- `Message` has an optional assistant-only `reasoning_content` string beside
  its typed `content`. `MessageContent` has no reasoning variant.
- The compatible Chat adapter concatenates non-empty streamed reasoning deltas
  without separators, persists the resulting string once, and maps it directly
  back to `reasoning_content` on later Chat requests.
- Visible text, final output, and history search ignore the sibling field.
  Exact history reads retain it, and compaction sizing counts it.
- Responses encrypted reasoning remains an ordered `ProviderItem`. Anthropic
  projection does not receive compatible Chat reasoning, and live reasoning
  deltas remain transient events.
- The pre-release run record advances to version 12. There is no decoder or
  migration for earlier development runs.

## Consequences

- A persisted assistant line matches the meaningful structure of the Chat
  payload and can replay whitespace exactly without reconstructing reasoning
  from content blocks.
- The provider-neutral message names one documented Chat-compatible extension
  directly. This is narrower and clearer than a generic reasoning abstraction
  that would imply false portability across provider protocols.
- Existing version-11 development runs are rejected at the run boundary rather
  than failing later if they contain the removed reasoning block.

## Alternatives Considered

- **Keep reasoning content blocks and concatenate them during projection.**
  Rejected because multiple blocks require an invented separator and make exact
  continuation replay depend on reconstruction.
- **Put Responses and Chat reasoning in one generic ordered enum.** Rejected
  because Chat reasoning is a message sibling while Responses reasoning is an
  opaque ordered output item.
- **Merge reasoning into visible assistant text.** Rejected because it changes
  user-visible output, search behavior, and provider continuation semantics.
- **Decode the former block shape.** Rejected because Fiasco has no released-run
  compatibility requirement and a migration would create two durable shapes.

## Related Documents

- [Architecture](../architecture.md)
- [Configuration](../configuration.md)
- [Runtime model](../runtime-model.md)
- [ADR 0001: Durable messages and transient deltas](0001-durable-messages-transient-stream-deltas.md)
- [ADR 0032: Self-contained message log](0032-self-contained-message-log.md)
