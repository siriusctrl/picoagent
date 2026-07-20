# ADR 0007: Emit Compacted-History Guidance Only at Its Active Boundary

- Status: Superseded
- Date: 2026-07-17
- Refines: ADR 0004 (recovery-guidance placement and prompt rendering)

Superseded by ADR 0012, which stores and directly reuses the exact assistant
compacted-state message and no longer emits a synthetic history wrapper.

ADR 0008 replaces the generic Markdown reflow portion below. Folded YAML now
handles source wrapping for built-in agent prompts; dynamic reminder inputs are
preserved exactly. Compacted-history guidance placement remains accepted.

## Context

ADR 0004 kept the normal system prompt and tool schemas stable by registering
the history tools from the first call and putting recovery guidance in every
initial runtime reminder. The schema stability remains useful, but a warning
about `<compacted-history>` spends tokens and describes absent context before a
checkpoint exists.

Compile-time Markdown and project `AGENTS.md` files are also commonly wrapped at
an editor column. Passing those source-only line breaks through prompt assembly
can split one sentence at a non-semantic boundary, spend tokens, and make the
model-facing prompt harder to inspect.

## Decision

- The initial runtime reminder does not include compacted-history recovery
  guidance.
- `history_search` and `history_read` remain registered before the first normal
  call, and the sorted tool-schema set remains frozen for the run.
- When a checkpoint is active, request assembly emits one synthetic user
  message containing the recovery guidance immediately before the
  `<compacted-history>` block. This active-context message is not appended to
  the durable trajectory, and the original initial message is not rewritten.
- Prompt assembly joins soft-wrapped lines within Markdown prose paragraphs.
  It preserves semantic boundaries including blank lines, headings, list
  items, tables, explicit line breaks, indented content, and fenced code.

## Consequences

- Runs without a checkpoint no longer pay for or see irrelevant recovery prose.
- Once compaction occurs, the model sees the instruction at the exact boundary
  where it matters and can use the already-present history tools.
- The system prompt, initial message, and tool schemas remain byte-stable across
  one run; only the active compacted suffix changes after checkpoint creation.
- Authored Markdown can remain conventionally wrapped without reproducing
  arbitrary source line breaks in model requests.

## Alternatives Considered

- **Keep guidance in every initial reminder.** Rejected because it describes
  context that usually does not exist yet and spends tokens on every run.
- **Add the history tools only after compaction.** Rejected because changing
  schemas mid-run weakens cache reuse and complicates request interpretation.
- **Rewrite the persisted initial message after compaction.** Rejected because
  it breaks the append-only complete-message boundary.
- **Remove all newlines from assembled Markdown.** Rejected because headings,
  lists, code, tables, and paragraph boundaries carry meaning.

## Related Documents

- [ADR 0004: Stable normal prefix and core history tools](0004-stable-agent-prefix-and-core-history-tools.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Prompt assets](../../prompts/README.md)
