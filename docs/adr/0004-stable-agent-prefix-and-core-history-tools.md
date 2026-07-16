# ADR 0004: Keep the Normal Agent Prefix and Core History Tools Stable

- Status: Accepted
- Date: 2026-07-16
- Refines: ADR 0003 (conditional prompt and history-tool placement only)

## Context

ADR 0003 established append-only local compaction and exact history retrieval.
An initial implementation could vary compaction guidance in the system prompt
or expose history tools only when automatic compaction was enabled. Those
conditional changes make otherwise similar normal requests start with different
system/tool prefixes and reduce opportunities for provider KV-cache reuse.

The harness is currently optimized for its known internal workflows. It does
not need per-step tool enablement, but it still needs fixed internal profiles
and configuration-dependent integrations.

## Decision

- The compiled system prompt is invariant across normal agent calls. It does
  not change when `compaction.trigger_tokens` is configured or a checkpoint is
  created.
- Each normal run's initial runtime reminder contains the stable guidance that
  treats `<compacted-history>` as historical data and directs exact recovery
  through `history_search` followed by `history_read`.
- `history_search` and `history_read` are registered before each normal
  profile's first provider request regardless of the automatic compaction
  trigger. `trigger_tokens` controls checkpoint creation only.
- One run's tool registry is sorted and frozen before its first normal provider
  call. Tool schemas are not added or removed after compaction.
- Checkpoint generation remains a separate request profile with its own system
  prompt and no tools. It is not a mutation of a normal agent profile.
- Root, delegating/leaf GeneralTask, and MemoryMaintenance have explicit
  capability profiles; callers do not supply a generic per-run tool allowlist.
  A GeneralTask's variant is selected from its remaining delegation depth
  before it starts. Memory and delegation tools depend on configured memory and
  the selected profile, while optional `web_search` and MCP tools depend on
  startup configuration. The selected schemas are frozen for the run.

This refines ADR 0003 only for where recovery guidance and history schemas are
placed. ADR 0003's append-only messages, separate checkpoints, active-context
assembly, and retrieval semantics are unchanged.

## Consequences

- Normal calls can reuse a longer byte-stable system/tool prefix before and
  after local compaction.
- Normal runs with no checkpoint expose history tools with no compacted prefix
  to inspect. This small schema cost buys a stable capability surface.
- Runtime-specific instructions remain visible in the first durable user
  message without making the global system prompt vary.
- Summary calls and different fixed profiles can have different prefixes by
  design; cache stability is guaranteed within an assembled profile, not across
  unrelated request types or startup configurations.
- Changing optional runtime capabilities changes the next Root run's frozen
  schema set and can produce a different cache prefix.

## Alternatives Considered

- **Add history tools only after compaction.** Rejected because it changes the
  normal tool-schema prefix mid-run.
- **Add compaction guidance to the system prompt only when enabled.** Rejected
  because the trigger is runtime policy, not a reason to vary stable identity
  and operating rules.
- **Duplicate full tool descriptions in the system prompt.** Rejected because
  provider-native schemas are authoritative and duplication spends tokens and
  can drift.
- **Give every internal request every tool.** Rejected because checkpoint
  summarization must not call tools, and focused internal profiles still need
  intentional fixed capability boundaries.
- **Hot-reload tools during a run.** Rejected for the launch harness because it
  breaks schema stability and complicates durable request interpretation.

## Related Documents

- [ADR 0003: Append-only local compaction and history retrieval](0003-append-only-local-compaction-and-history-retrieval.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Configuration](../configuration.md)
- [Prompt assets](../../prompts/README.md)
- [Design choices](../design-choices.md)
