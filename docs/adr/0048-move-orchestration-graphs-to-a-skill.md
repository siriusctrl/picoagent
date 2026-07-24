# ADR 0048: Move Orchestration Graphs to an Agent Skill

- Status: Accepted
- Date: 2026-07-24
- Supersedes: ADR 0026, ADR 0031, and ADR 0040

## Context

The graph design began as a run-local tool family with allocation, validation,
status projection, and listing. As execution and recovery were deliberately
kept outside the graph, the remaining value became procedural: deciding when a
graph helps, choosing useful nodes and dependencies, integrating accepted
knowledge, and revising the model as the task changes.

Those decisions depend on model judgment rather than deterministic runtime
behavior. Keeping them in `graph_init`, `graph_list`, a Rust document model, and
large tool descriptions made an optional planning method part of every run's
fixed capability surface. It also made the method harder to install, ablate,
and iterate independently.

## Decision

- Remove the built-in `graph_init` and `graph_list` tools, shared graph store,
  Rust graph document model, validation, and run-local graph directory.
- Ship `skills/orchestrate-with-graphs/` as an installable Agent Skill
  discoverable both by Fiasco and standard skill tooling.
- Keep graph files as lightweight workspace YAML under `.agents/graphs/`.
  Ordinary file capabilities provide discovery, reading, creation, and edits.
- Treat a graph as the orchestrator's externalized mental model. Nodes express
  outcomes, questions, decisions, and dependencies; they do not correspond
  one-to-one with agents or execution attempts.
- Use only `open` and `resolved` graph lifecycle. A resolved graph may retain
  unresolved nodes when its summary explains abandonment, supersession, or a
  narrowed objective.
- Keep execution, agent handles, retries, crash recovery, and scheduling outside
  graph content. An agent completion becomes a resolution only after the
  orchestrator accepts its result.
- Add a remote graph access capability only when shared remote storage becomes
  a concrete requirement. Do not emulate its concurrency or recovery protocol
  in local files.

## Consequences

- The default model-facing tool set loses two schemas and the runtime loses the
  complete graph-specific implementation and test surface.
- Graph guidance can evolve through Markdown, load progressively, and be
  installed or removed independently of the Fiasco binary.
- Workspace graphs can be reused across runs and inspected by other tools or
  people without run-directory knowledge.
- There is no built-in graph validation, collision-free id allocator, grouped
  listing, or readiness projection. The skill favors descriptive file names,
  readable conventions, and model judgment instead.
- Malformed or stale graphs are ordinary workspace-file problems rather than a
  new runtime recovery state.

## Alternatives Considered

### Keep minimal initialization and listing tools

Rejected for now because local file creation and discovery are already ordinary
capabilities. Retaining special tools would preserve most of the product
surface while moving only prose.

### Keep the graph workflow in stable prompts or tool descriptions

Rejected because graph planning is optional. A skill gives it an explicit
trigger, progressive loading, independent installation, and clean ablation.

### Build remote graph storage now

Rejected because no current requirement needs cross-machine access or remote
concurrency. A future service should expose the access semantics it actually
needs rather than inherit a speculative local protocol.

## Related Documents

- [Graph-based orchestration skill](../../skills/orchestrate-with-graphs/SKILL.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
