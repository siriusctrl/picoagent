# ADR 0009: Maintain Memory Through Ordinary Tools

- Status: Accepted
- Date: 2026-07-17
- Refines: ADR 0004 (memory-specific capability profile only)
- Refines: ADR 0008 (memory prompt inventory only)

## Context

Memory is already ordinary Markdown at paths included in each normal run's
runtime reminder. The dedicated `memory_update` tool wrapped the same `read`,
`write`, and `bash` operations in a synchronous child run, which required a
MemoryMaintenance profile, two extra prompt fields, special capability
assembly, timeout cleanup, recovery documentation, and separate tests.

That machinery did not add storage isolation, a different mutation primitive,
or a semantic ability unavailable to the normal agent. The harness values
simplicity and inspectability over a broader specialized API.

## Decision

- Do not register a memory-specific model tool or maintain a
  MemoryMaintenance run profile.
- When memory is enabled, put only the resolved user and project Markdown paths
  in every normal root or GeneralTask reminder.
- Keep general memory practices in the invariant system prompt: inspect before
  editing, separate user and project scopes, curate durable facts, and avoid raw
  transcript dumps.
- Use `read`, `write`, and `bash` directly for small focused updates.
- Use the existing asynchronous GeneralTask path for a large independent
  consolidation. It receives the same memory paths and follows the ordinary
  durable child-run and parent-resume contract.
- Keep `fiasco memory consolidate` as a convenience command, implemented as an
  ordinary root task that may delegate and verifies the consolidation.

## Consequences

- Memory configuration changes reminder text but no longer changes tool
  schemas. Root and GeneralTask have fewer profile-specific branches.
- Small updates avoid an unnecessary provider call and child transcript.
- Large updates remain auditable and resumable through the same task machinery
  as every other GeneralTask child.
- Model judgment still decides what is durable; the Rust harness owns only
  paths and ordinary execution contracts.
- Pre-release runs stored with the removed `memory_maintenance` profile are not
  resumed by the new binary.

## Alternatives Considered

- **Keep `memory_update` as a synchronous direct tool.** Rejected because it
  duplicates ordinary file operations and introduces special child lifecycle
  semantics.
- **Keep a dedicated profile only for the consolidation command.** Rejected
  because a normal root plus GeneralTask already provides the required scope,
  persistence, and verification flow.
- **Let Rust automatically extract memory after every run.** Rejected because
  transcripts and successful outcomes are evidence, not automatically curated
  durable knowledge.
- **Add a database or vector index.** Rejected until a concrete retrieval need
  exceeds direct Markdown reads and `rg`.

## Related Documents

- [Memory](../memory.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [ADR 0004](0004-stable-agent-prefix-and-core-history-tools.md)
- [ADR 0008](0008-typed-agent-prompt-registry.md)
