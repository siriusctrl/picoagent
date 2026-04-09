# Architecture

## Current Scope

The repository is intentionally a single TypeScript package with one real product surface: the ACP server.

That is the intended shape at this scale:

- one core runtime
- one primary transport
- optional thin clients
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

The product surface.

Responsibilities:

- stdio ACP entrypoint
- ACP session lifecycle
- ACP-backed environment for file IO and command execution
- mapping tool execution into ACP session updates

Rules:

- may depend on `core`, `bootstrap`, `config`, `fs`, and `prompting`
- owns transport details, not model logic
- should remain stable enough that clients can stay disposable

### `src/clients`

Replaceable clients.

Responsibilities:

- host thin shells around the ACP server
- keep local UX concerns out of the runtime core

Current client:

- `src/clients/tui`
  - spawns the ACP server locally
  - implements ACP client capabilities for filesystem and terminals
  - renders a terminal-native inspection UI

Rules:

- keep it thin
- do not move model logic here
- do not let TUI constraints drive core architecture

### `src/bootstrap`

Runtime assembly only.

Responsibilities:

- load config from the control workspace
- create the provider
- assemble the general tool registry

### `src/config`

Configuration helpers.

Responsibilities:

- load workspace and user config
- resolve provider auth from environment variables

### `src/fs`

Deterministic filesystem helpers.

Responsibilities:

- resolve session-safe paths
- traverse files
- perform text search

### `src/prompting`

Prompt framing helpers.

Responsibilities:

- frontmatter-backed prompt scanning
- system prompt assembly

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

## Dependency Rules

- `core` must stay independent of provider SDKs, ACP, and Ink
- `providers` may depend on `core`, but not vice versa
- `tools` may depend on `core` and `fs`
- `acp` may depend on `core`, `tools`, `bootstrap`, `fs`, and `prompting`
- `clients` should depend on ACP behavior and local client concerns only
- `bootstrap` may depend on `core`, `config`, `providers`, and `tools`

## Product Rule

Default sequence for changes:

1. `src/core`
2. `src/acp`
3. thin client updates only if needed

If a feature only exists to make the TUI nicer, that is usually not a good enough reason to reshape the system.

## Tool Model

There is one tool registry for the whole app.

That registry owns:

- the full set of tools
- which tools each mode equips

Current modes:

- `ask`
- `exec`

The important rule is:

- tools are general
- modes are curated subsets

Do not hard-code mode behavior in multiple layers if the registry boundary is enough.
