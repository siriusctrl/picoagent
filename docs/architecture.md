# Architecture

## Current Scope

The repository is intentionally a single TypeScript package.

That is a deliberate end state for the current scale:
- one runtime target
- one UI target
- one provider surface
- cheap cross-cutting refactors

## Code Boundaries

### `src/core`

The kernel.

Responsibilities:
- message and tool types
- provider contract
- tool registry
- agent loop

Rules:
- no provider SDK imports
- no ACP-specific types
- no Ink or terminal UI code

### `src/acp`

The ACP agent adapter.

Responsibilities:
- stdio ACP entrypoint
- ACP session lifecycle
- ACP-backed environment for file IO and command execution
- mapping tool execution into ACP session updates

Rules:
- may depend on `core`, `app`, and `lib`
- owns transport details, not model logic

### `src/tui`

The local client.

Responsibilities:
- spawn the ACP agent locally
- implement ACP client capabilities for filesystem and terminals
- render a terminal-native UI with Ink

Rules:
- should stay a thin client
- should not know provider internals

### `src/providers`

SDK adapters only.

Responsibilities:
- translate between `src/core` message/tool shapes and provider SDKs
- stream deltas back into the core loop

### `src/tools`

LLM-facing capabilities.

Responsibilities:
- define tool parameters
- validate tool arguments
- call into the environment or filesystem helpers

### `src/lib`

Shared helpers.

Responsibilities:
- config loading
- prompt assembly
- frontmatter parsing
- filesystem traversal and search

### `src/app`

Bootstrap only.

Responsibilities:
- load config from the control workspace
- create the provider
- assemble the general tool registry

## Dependency Rules

- `core` must stay independent of provider SDKs, ACP, and Ink.
- `providers` may depend on `core`, but not vice versa.
- `tools` may depend on `core` and `lib`.
- `acp` may depend on `core`, `tools`, `app`, and `lib`.
- `tui` may depend on `acp` only through the ACP protocol and local client behavior.
- `app` may depend on everything except `tui`.

## Tool Model

There is one tool registry for the whole app.

That registry owns:
- the full set of tools
- which tools each mode equips

Current modes:
- `ask`
- `exec`

The important design rule is:
- tools are general
- modes are just curated subsets

Do not hard-code mode behavior in multiple layers if the registry boundary is enough.
