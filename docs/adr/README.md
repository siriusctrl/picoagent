# Architecture Decision Records

Architecture Decision Records (ADRs) explain why a durable technical choice was
made. Runtime contracts remain in their topic documents; ADRs preserve the
decision context, credible alternatives, and consequences that would otherwise
be lost in commits or chat history.

## When To Write One

Add an ADR when a change establishes or revises a cross-module invariant,
persistence or provider contract, security boundary, or other costly-to-reverse
choice with credible alternatives. Ordinary bug fixes, local refactors, and
straightforward feature additions do not need an ADR.

## Workflow

1. Create `NNNN-short-title.md` using the next four-digit number.
2. Start with `Proposed`; change it to `Accepted` when the decision is adopted.
3. Treat accepted ADRs as historical records. Fixing typos is fine, but a
   replaced decision requires a new ADR marked `Supersedes: ADR NNNN`; mark the
   old ADR `Superseded by ADR NNNN`. For a narrowly scoped change, use
   `Refines: ADR NNNN (scope)`, leave the old record accepted, and add an
   explicit forward note without rewriting its original decision.
4. Add the record to the index below and link it from the relevant contract or
   `docs/design-choices.md` summary.

Use these sections:

```text
# ADR NNNN: Title

- Status: Proposed | Accepted | Rejected | Superseded
- Date: YYYY-MM-DD
- Supersedes: ADR NNNN (when applicable)
- Refines: ADR NNNN (scope, when applicable)

## Context
## Decision
## Consequences
## Alternatives Considered
## Related Documents
```

## Index

- [ADR 0001: Persist complete messages and keep stream deltas
  transient](0001-durable-messages-transient-stream-deltas.md)
- [ADR 0002: Embed prompt assets and keep tools with their
  owners](0002-compile-time-prompt-assets-and-tool-ownership.md)
- [ADR 0003: Add local compaction without rewriting the
  trajectory](0003-append-only-local-compaction-and-history-retrieval.md)
- [ADR 0004: Keep the normal agent prefix and core history tools
  stable](0004-stable-agent-prefix-and-core-history-tools.md)
