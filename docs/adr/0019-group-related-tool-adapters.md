# ADR 0019: Group Related Tool Adapters Without Deriving Names

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0014 (local adapter source layout)

## Context

Putting every model-facing adapter directly under `src/tools/<tool>/` made
ownership easy to find when the set was small. The task controls and history
retrieval tools now form two cohesive families with repeated prefixes, shared
dependencies, and all-or-nothing registration. Keeping every member flat made
the top-level module and run assembly repeat those families rather than clarify
them.

The source tree should express those relationships without making filesystem
paths an implicit part of the model-facing protocol.

## Decision

- Keep standalone adapters directly under `src/tools/<tool>/`.
- Group cohesive families as `src/tools/<family>/<member>/`. Initially these
  are `task/{inspect,status,steer,stop,wait}` and `history/{read,search}`.
- Each leaf `tool.yaml` continues to declare the complete model-facing name,
  such as `task_status` or `history_search`. Directory names never generate or
  modify tool names.
- Each family module owns explicit registration of all its members.
  `RunToolAssembly` delegates to those family functions instead of constructing
  every adapter itself.
- Domain engines remain outside the adapter tree: task state stays in
  `TaskManager`, and history access stays behind `TrajectoryReader`.

## Consequences

- The top-level `src/tools` namespace stays short while related adapters and
  their shared helpers are adjacent.
- A maintainer can still find the exact provider-visible name by opening or
  searching the leaf manifest.
- Moving source directories cannot silently rename a capability or change a
  persisted schema hash.
- Adding a family member requires an explicit entry in its family registration
  function and in the manifest inventory test.

## Alternatives Considered

- **Derive `task_` or `history_` from the directory path.** Rejected because
  the leaf manifest would no longer show the actual provider-visible contract,
  searches for the full name would miss its definition, and moving code could
  rename a capability.
- **Keep every adapter flat.** Rejected because the repeated family prefixes
  and assembly statements now obscure useful relationships.
- **Discover and register directory contents automatically.** Rejected because
  compile-time embedding still needs explicit assets, while filesystem
  discovery would add build machinery and make capability membership less
  obvious.

## Related Documents

- [Architecture](../architecture.md)
- [Source map](../source-map.md)
- [Prompt assets](../../prompts/README.md)
- [ADR 0014](0014-flat-tool-adapters-and-explicit-assembly.md)
- [ADR 0015](0015-local-tool-yaml-manifests.md)
