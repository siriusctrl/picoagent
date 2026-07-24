# ADR 0031: Validate the complete initial graph topology before creation

- Status: Superseded
- Date: 2026-07-21
- Refines: ADR 0026 (planning graph initialization)

Superseded by ADR 0048, which removes graph-specific initialization and
validation from the runtime.

ADR 0040 refines the accepted input from a goal plus node map to the exact
complete graph document, including already accepted resolutions.

## Context

Creating an empty graph and asking the model to fill its nodes with a later
`write` call required an extra model turn and temporarily left a meaningless
but valid `wip` graph. It also delayed dependency and cycle validation until a
separate `graph_list` call.

## Decision

`graph_init` accepts the goal and complete initial node map in one call. Every
node supplies an objective and dependency ids. The tool constructs version-1
`wip` state with null resolutions, validates ids, references, duplicates, and
acyclicity, then starts creating the YAML only if validation succeeds. Its
result reports the graph id, path, counts, and initially ready nodes. The tool
description and JSON Schema include a representative topology example.

This decision guarantees validation-before-create, not transactional file
publication across process crashes. The graph store keeps its existing simple
`create_new` write path and never overwrites an existing graph.

Later plan changes still use ordinary `write`; `graph_list` remains the
validator and readiness view after edits. Graph execution remains separate and
uses `delegate` plus task controls.

## Consequences

- The initial durable file always contains an intentional validated topology.
- Invalid initialization creates no partial graph file.
- Initialization stays one tool action while later changes keep using ordinary
  file tools.

## Alternatives Considered

- **Create an empty skeleton.** Rejected because it adds a write turn and
  exposes incomplete topology as a valid graph.
- **Add graph-specific mutation tools.** Rejected because `write` and
  `graph_list` already cover revision and validation.

## Related Documents

- [Architecture](../architecture.md)
- [ADR 0026](0026-file-backed-planning-graphs.md)
