# Architecture

## Current Scope

The repository is intentionally a single TypeScript package with one core runtime, one thin HTTP transport adapter, and thin local clients.

That is the intended shape at this scale:

- one core runtime
- one minimal HTTP adapter
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
- no transport-specific types
- no Ink or terminal UI code

### `src/http`

The minimal local HTTP adapter.

Responsibilities:

- expose async run and session resources
- expose run events as JSON or SSE
- project snapshots from the runtime store
- reuse the same bootstrap path and agent loop as other transports

Rules:

- keep the surface narrow
- keep resource boundaries explicit: session, run, events
- do not let request handlers become the source of truth for runtime state
- do not fork the runtime model away from `core`

### `src/clients`

Replaceable clients.

Responsibilities:

- host thin shells around the HTTP surface
- keep local UX concerns out of the runtime core

Current client:

- `src/clients/cli`
  - exposes a minimal command-line service surface
  - starts the HTTP server
- `src/clients/tui`
  - starts the HTTP server locally
  - renders a terminal-native smoke-test UI

Rules:

- keep it thin
- do not move model logic here
- do not let TUI constraints drive core architecture

### `src/bootstrap`

Runtime assembly only.

Responsibilities:

- assemble the general tool registry
- build session control snapshots from the bound workspace

### Runtime state

Runtime state should sit behind explicit store boundaries rather than live in HTTP handlers.

Current shape:

- an in-memory runtime store owns sessions, runs, subscriptions, and append-only run events
- each session stores a cached control snapshot derived from its bound workspace
- sessions may also store checkpoints that compact older conversation history into summaries
- HTTP reads snapshots and event streams from that store
- clients observe projections, not handler-local state
- session history can be exposed back to the model as virtual session resources instead of being forced into the prompt every time

### `src/config`

Configuration helpers.

Responsibilities:

- load workspace and user config
- resolve provider auth from environment variables

### `src/fs`

Deterministic filesystem helpers.

Responsibilities:

- define the workspace filesystem boundary used by tool-facing environments
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
- expose session history browsing or compaction through small focused tools when the runtime model needs it

## Runtime Hands

The agent "hands" are represented by the environment boundary passed into the runtime.

Rules:

- the harness should depend on the `AgentEnvironment` interface, not one concrete local implementation
- file-backed behavior inside that environment should depend on the `WorkspaceFileSystem` boundary
- local filesystem and command execution are only the default implementation
- future remote sandboxes should be a replacement implementation, not a rewrite of the HTTP layer

## Dependency Rules

 - `core` must stay independent of provider SDKs, HTTP, and Ink
- `providers` may depend on `core`, but not vice versa
- `tools` may depend on `core` and `fs`
- `http` may depend on `core`, `tools`, `bootstrap`, `fs`, and `prompting`
- `clients` should depend on HTTP behavior and local client concerns only
- `bootstrap` may depend on `core`, `config`, `providers`, and `tools`

## Product Rule

Default sequence for changes:

1. `src/core`
2. transport adapter updates only if needed
3. thin client updates only if needed

If a feature only exists to make the TUI nicer, that is usually not a good enough reason to reshape the system.

## Tool Model

There is one tool registry for the whole app.

That registry owns:

- the full set of tools
- which tools each agent preset equips

Current built-in agent presets:

- `ask`
- `exec`

The important rule is:

- tools are general
- agent presets are curated subsets

Do not hard-code agent behavior in multiple layers if the registry boundary is enough.
