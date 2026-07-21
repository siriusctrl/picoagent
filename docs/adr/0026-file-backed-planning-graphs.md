# ADR 0026: Keep planning graphs file-backed and separate from task execution

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0017 (complex-task planning remains separate from task execution)

ADR 0031 later refines initialization: `graph_init` receives and validates the
complete initial topology before creating the file. File-backed revision,
validation, readiness, and separation from task execution remain accepted.

## Context

Long tasks benefit from a durable dependency graph that the main agent can
revise as results and user input arrive. Treating each graph node as an agent or
letting a scheduler automatically run every ready successor would couple the
plan to one execution attempt. It would also duplicate the existing delegated
task lifecycle and make it harder for the main agent to review evidence before
accepting progress.

The graph must remain inspectable and resumable with the rest of a run. This
internal harness favors ordinary files and existing tools over a new database,
patch protocol, or workflow engine.

## Decision

Each graph is a versioned YAML file at
`.fiasco/runs/<run-id>/graphs/g<N>.yaml`, where `g<N>` is a short sequential
run-local id. `graph_init({goal})` safely allocates a new version-1 `wip`
skeleton without overwriting an existing file. `graph_list({})` parses every
graph independently and groups valid summaries as `wip`, `completed`, or
`aborted`; malformed or inconsistent files remain visible under `invalid`.

A node is a durable work item with an objective, dependency ids, and a
null-or-object resolution. A resolution contains a non-empty summary and
optional project-relative evidence paths. It records an outcome accepted by the
main agent, not merely a completed agent run. Readiness is derived: an
unresolved node in a `wip` graph is ready exactly when all dependencies are
resolved. Terminal graphs expose no ready nodes, and a node cannot retain an
accepted resolution while any direct dependency is unresolved.

The validator rejects unknown or repeated dependencies, cycles, unsafe evidence
paths, and inconsistent terminal state. `completed` requires all nodes resolved
and a top-level summary; `aborted` requires an abort reason. Full inspection and
mutation reuse `read` and `write`. Executing ready work reuses `delegate`, and
running work reuses the task controls. Graph files contain no task ids and do
not launch work automatically. Because calls in one assistant tool-call batch
are concurrent, a dependent graph edit, validation listing, and delegation must
occur in separate assistant turns.

## Consequences

- The main agent can revise pending topology between execution waves and decide
  when evidence is sufficient to resolve a node.
- Graph files remain portable, human-readable run output and survive resume
  without another persistence subsystem.
- Tool schemas stay small: there is no graph update, dispatch, resolve, wait, or
  stop family.
- Task completion does not update the graph automatically. The main agent must
  integrate results, write resolutions, and call `graph_list` after meaningful
  edits to validate and derive the next ready work.
- Graph and task state can temporarily diverge. This is an accepted simplicity
  tradeoff rather than a reason to couple the two stores preemptively.

## Alternatives Considered

### Make one graph node one agent

Rejected because retries, multiple reviews, direct main-agent work, and
acceptance of an outcome are all distinct from one execution attempt.

### Add graph mutation and execution tools

Rejected because a patch DSL and graph-specific dispatch/control lifecycle
would duplicate `write`, `delegate`, and the task tools.

### Automatically launch ready successors

Rejected because the main agent must have a boundary to integrate results,
incorporate new user information, and revise the topology before the next wave.

### Store graphs in task records or a database

Rejected because planning topology is not task state, and current runs are
already self-contained filesystem outputs.

## Related Documents

- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
- [ADR 0017](0017-concurrent-tool-batches-and-explicit-task-controls.md)
