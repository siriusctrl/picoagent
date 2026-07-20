# ADR 0002: Embed Prompt Assets and Keep Tools With Their Owners

- Status: Superseded
- Date: 2026-07-15

Superseded by ADR 0014.

ADR 0008 refined the agent-prompt packaging below. ADR 0014 later superseded
this combined ownership decision: compile-time assets remain, but every local
model-facing adapter now lives under `src/tools/` while domain logic stays in
its focused subsystem.

## Context

Stable system instructions and tool descriptions were inline Rust strings.
This made model-facing prose difficult to discover and review independently
from prompt assembly, schemas, validation, and execution logic. The standalone
base tools also lived beneath a `builtin/` category with no sibling tool family,
adding navigation without expressing a useful ownership boundary.

Prompt and tool assets must remain deterministic and easy to package in the
single picoagent binary. A model-visible schema must not drift from the Rust
arguments and behavior that implement it. Tools coupled to task supervision,
memory, skills, or MCP also should not move into a central module merely because
they implement the common `Tool` trait.

## Decision

- Stable agent-level prose lives as Markdown under `prompts/agents/` and is
  embedded into the binary with `include_str!`.
- Rust owns prompt precedence, dynamic values, section ordering, and envelope
  construction. Project instructions continue to come from `AGENTS.md`.
- Each standalone base tool lives in a flat `src/tools/<tool>/` module with its
  `description.md` beside the Rust implementation.
- Tool names, JSON schemas, argument deserialization, validation, and execution
  remain Rust contracts in the owning module.
- Static descriptions for subsystem tools live beside their owning subsystem;
  their implementation is not moved into `src/tools/`.
- MCP remains the extension boundary for externally packaged executable tools.
  Picoagent does not scan directories or load a native dynamic plugin ABI.
- Prompt and description assets are compile-time inputs. Runtime customization
  or replacement is a separate product decision.

## Consequences

- Prompt-only changes have focused Markdown diffs while producing a
  self-contained binary with no runtime asset lookup.
- Missing assets fail compilation, and asset changes trigger a rebuild.
- Built-in tool navigation is flatter, while subsystem dependency direction
  remains intact.
- Changing embedded prose still requires rebuilding picoagent.
- Source packaging must include the referenced Markdown assets; release checks
  should inspect `cargo package --list`.

## Alternatives Considered

- **Keep prose inline in Rust.** Rejected because it mixes frequently reviewed
  natural language with assembly and execution logic.
- **Centralize every tool under `src/tools/`.** Rejected because orchestration,
  memory, skill, and MCP tools belong to and depend on their subsystems.
- **Load prompt and tool files dynamically at runtime.** Rejected for the core
  assets because it adds deployment failure modes and weakens deterministic
  prompt prefixes.
- **Add a manifest or native plugin folder per tool.** Rejected because it
  duplicates Rust schemas and introduces a plugin system without a concrete
  requirement. External tools already have MCP.

## Related Documents

- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
- [Source map](../source-map.md)
- [Design choices](../design-choices.md)
