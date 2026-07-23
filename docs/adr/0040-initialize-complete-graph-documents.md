# ADR 0040: Initialize Complete Graph Documents

- Status: Accepted
- Date: 2026-07-23
- Refines: ADR 0031 (complete initial graph topology)

## Context

Passing only a goal and node topology made `graph_init` construct part of the
persisted YAML implicitly. It also forced an immediate file edit when the model
already had accepted conclusions and evidence that belonged in the initial
graph. The initialization call could validate topology, but it could not
validate the exact complete document the model intended to create.

## Decision

- Accept one `graph` argument containing the complete version-1 `wip`
  `GraphDocument`.
- Require every node to provide `objective`, `depends_on`, and `resolution`.
  A resolution is either null or the complete accepted summary and optional
  evidence.
- Apply the ordinary graph validator before allocating a graph file. Initial
  resolved nodes must satisfy the same dependency rule as later edits.
- Persist the validated document directly. Invalid input creates no file.
- Keep later inspection and mutation with ordinary file capabilities and keep
  graph execution outside the graph store.

## Consequences

- The schema example is also a complete inspectable example of the persisted
  graph format.
- Initialization can capture both planned work and accepted knowledge in one
  tool call.
- Rust no longer normalizes a second, partial initialization shape into the
  durable model.
- Callers must state the fixed `version` and initial `wip` status explicitly.

## Alternatives Considered

- **Keep goal plus node map and add optional initial results.** Rejected because
  it preserves a second near-duplicate graph representation.
- **Accept raw YAML text.** Rejected because structured arguments give providers
  a precise schema and reject unknown fields before persistence.
- **Create first and validate with a later listing call.** Rejected because an
  invalid graph would briefly become durable.

## Related Documents

- [ADR 0026](0026-file-backed-planning-graphs.md)
- [ADR 0031](0031-validate-complete-initial-graph-topology.md)
- [Architecture](../architecture.md)
