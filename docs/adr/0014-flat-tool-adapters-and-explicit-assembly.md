# ADR 0014: Flat Tool Adapters and Explicit Assembly

- Status: Accepted
- Date: 2026-07-20
- Supersedes: ADR 0002

ADR 0015 later refines only the local tool contract packaging: a per-tool typed
YAML manifest now owns the static name, description, and input schema. The
other decisions in this record remain accepted.

ADR 0017 later removes the explicit ordinary-tool spawn policy and its dynamic
schema. Ordinary tools are direct capabilities with automatic foreground
promotion; `delegate` and the five task controls are separate flat adapters.
The common ownership and single assembly decisions here remain accepted.

ADR 0035 later expands and revises the task-control leaves for reusable agents;
they remain flat adapters with explicit assembly.

ADR 0019 later refines only the source layout: cohesive task and history
families are grouped beneath `src/tools/<family>/`, while each leaf adapter
still owns its complete manifest and registration remains explicit.

ADR 0038 later removes the durable `TaskManager` and replaces the task-control
family with runtime-handle adapters under `src/tools/handle/`. Explicit
assembly, leaf-owned manifests, and domain-engine separation remain accepted.

ADR 0049 replaces the dynamic per-remote-tool MCP adapter with one fixed
command adapter backed by progressive MCP artifacts. MCP lifecycle and
domain-engine separation remain accepted.

## Context

Model-facing tools had two ownership patterns. Base and history adapters lived
under `src/tools/`, while skill and task adapters lived inside their domain
subsystems. Static registration was split between `main` and `AgentRunner`, and
the registry snapshot taken for `TaskManager` implicitly decided which tools
could be named through `spawn(kind=tool)`. Understanding one model-visible
contract therefore required following directory conventions and statement
order across several modules.

The prompt-asset part of ADR 0002 remains useful, but its rejection of a common
adapter location now works against the project's priority of simplicity and
understandability.

## Decision

- Every local model-facing adapter lives in a flat `src/tools/<name>/` module
  with its Rust schema, validation, thin execution adapter, and compile-time
  Markdown description.
- Domain engines stay focused: `TaskManager`, `SkillRegistry`, and
  `TrajectoryReader` remain outside `src/tools/`. MCP lifecycle and its dynamic
  adapter remain in `mcp.rs`.
- `build_app_tools` and `RunToolAssembly` form the single static assembly path.
  The composition root may add configured MCP adapters, but does not register
  local tools individually.
- Every registry insertion explicitly allows or denies direct model use through
  `spawn(kind=tool)`. Foreground-timeout promotion is a separate lifecycle and
  remains available to any directly called tool.
- The `spawn` input schema lists the exact allowed tool names as an enum. The
  frozen model schema and its persisted resume hash therefore include this
  capability decision.
- Stable agent prose stays in the typed compile-time YAML registry. Tool-specific
  behavior and output contracts stay in each tool description rather than the
  system prompt.

## Consequences

- A maintainer can find every local model-visible schema and description from
  one flat directory without moving the domain state machines there.
- Adding a local tool requires one explicit assembly decision and one explicit
  background policy.
- Specs are generated and cached once at registration, so schema generation
  cannot drift between filtering, hashing, and provider requests.
- The `src/tools` module may depend on domain interfaces for thin adapters; this
  is intentional and does not move domain behavior into the adapter layer.
- Changing the explicit background set changes the `spawn` schema and causes an
  incompatible resume to fail capability validation.

## Alternatives Considered

- **Keep subsystem adapters beside domain state.** Rejected because the split
  made model-visible contracts harder to discover and assembly harder to audit.
- **Put domain implementations in each tool folder.** Rejected because it would
  mix persistent task/skill/trajectory behavior into transport adapters.
- **Keep implicit snapshot ordering.** Rejected because statement order is not
  a readable capability policy and cannot be explained by the schema.
- **Move schemas and descriptions to YAML manifests.** Rejected because Rust
  validation and execution would still be authoritative, creating two contracts
  and plugin-like packaging without a concrete need.

## Related Documents

- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
- [Source map](../source-map.md)
- [Design choices](../design-choices.md)
