# Graph Format

Store one YAML document per graph under `.agents/graphs/`. This format is a
shared convention for agents and people, not runtime state or a scheduler.

## Document Shape

```yaml
status: open
goal: Ship image input support with a verified provider contract
nodes:
  inspect_contract:
    objective: Determine the provider request contract
    depends_on: []
    resolution:
      summary: The provider accepts native image content blocks
      evidence:
        - docs/provider-contract.md
        - child run 01J...
  implement:
    objective: Implement the accepted contract
    depends_on: [inspect_contract]
    resolution: null
  verify:
    objective: Verify text and image requests end to end
    depends_on: [implement]
    resolution: null
```

Use these fields:

- `status`: use `open` while the graph needs attention and `resolved` when it
  does not.
- `goal`: state the coherent objective or question represented by the graph.
- `summary`: omit while open; require a concise explanation when resolved.
- `nodes`: map stable, descriptive node ids to objectives and relationships.
- `objective`: describe the outcome or question, not the execution mechanism.
- `depends_on`: list node ids whose conclusions are prerequisites.
- `resolution`: use `null` until a conclusion is accepted; otherwise record a
  non-empty `summary` and optional `evidence`.
- `evidence`: use short, non-empty references such as paths, URLs, run ids,
  commands, or observations. Evidence is not restricted to files.

Keep dependencies free of unknown ids, duplicates, and cycles. Accept a node
resolution only after its direct dependencies are resolved. These rules keep
the graph intelligible; do not build extra validation machinery around them.

## Resolving a Graph

A resolved graph may still contain unresolved nodes:

```yaml
status: resolved
goal: Choose a storage backend for shared orchestration graphs
summary: Deferred remote storage; workspace YAML is sufficient for current use
nodes:
  local_files:
    objective: Determine whether workspace files satisfy current coordination
    depends_on: []
    resolution:
      summary: Ordinary file tools cover current discovery and editing needs
      evidence:
        - src/tools/read
        - src/tools/write
  remote_server:
    objective: Design remote access and concurrency semantics
    depends_on: [local_files]
    resolution: null
```

This is valid because the top-level summary explains why the remaining branch
does not require current attention.

## Evolution

Edit the whole document or make a precise file patch. Add fields only when they
clarify the orchestrator's current reasoning; never add queues, leases,
heartbeats, retry counters, or other runtime state.

If a future remote graph service supplies dedicated access tools, use those
tools for storage and discovery while preserving these mental-model semantics.
Do not make the file convention emulate a remote coordination protocol in
advance.
