# ADR 0008: Consolidate Agent Prompts in a Typed YAML Registry

- Status: Accepted
- Date: 2026-07-17
- Refines: ADR 0002 (agent-prompt packaging only)
- Refines: ADR 0007 (source-wrap handling only)

ADR 0009 removes the two memory-specific prompt fields. The typed YAML registry
and folding decision remain accepted.

## Context

Six short fixed agent prompts lived in six Markdown files and were included by
five Rust modules. Preserving readable source wrapping then required generic
Markdown reflow code during request assembly. Picoagent is an internal harness,
so direct editing and a single obvious inventory matter more than independent
packaging for each short prompt.

## Decision

- All fixed agent-level prompts live as flat named fields in
  `prompts/agents.yaml`.
- Each value uses YAML's folded, stripped scalar form (`>-`). Source wrapping is
  removed by YAML parsing; blank source lines retain semantic boundaries.
- `src/prompts.rs` deserializes the embedded registry once into a typed struct,
  rejects unknown and empty fields, and exposes it to agent profiles.
- Invalid embedded YAML is a programmer error checked by tests and fails fast
  when the registry is first loaded.
- Project `AGENTS.md`, skill metadata, memory paths, delegated instructions,
  tool descriptions, and subsystem-specific model guidance keep their existing
  owners and are not moved into the agent registry.

## Consequences

- One file shows every fixed agent profile and makes cross-profile editing easy.
- YAML performs prose folding, so prompt assembly no longer contains a Markdown
  reflow implementation.
- Adding or renaming an agent prompt requires changing both the YAML and typed
  Rust structure; missing, unknown, and empty values fail tests.
- The binary gains one YAML parser dependency but still performs no runtime file
  lookup and keeps deterministic embedded prompts.

## Alternatives Considered

- **Keep one Markdown file per prompt.** Rejected because the current prompts
  are small and easier to compare in one internal registry.
- **Keep generic Markdown reflow in Rust.** Rejected because YAML folding states
  the same intent in the asset and removes parsing policy from assembly code.
- **Use a build script to generate Rust constants.** Rejected because it adds a
  second compilation phase for a tiny registry; parse-once startup cost is
  negligible.
- **Move tool descriptions into the same YAML.** Rejected because each tool owns
  its schema, implementation, and model-facing description.

## Related Documents

- [ADR 0002: Prompt assets and tool ownership](0002-compile-time-prompt-assets-and-tool-ownership.md)
- [ADR 0007: Compacted-history guidance placement](0007-compacted-history-guidance-at-active-boundary.md)
- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
