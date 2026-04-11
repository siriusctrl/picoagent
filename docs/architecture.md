# Architecture

Read with:

- `README.md`
- `docs/design-choices.md`
- `docs/golden-principles.md`

## Goal

`picoagent` is a small, controllable agent harness.

The architectural goal is to keep four concerns explicit:

- `session` stores context
- `runtime` executes one run through the shared loop
- `filesystem` provides file-backed surfaces
- `execution backend` runs commands

## Current Scope

The repository is intentionally one TypeScript package with:

- one core runtime
- one thin HTTP adapter
- thin local clients

The local host stack is Bun-native, but Bun stays behind those boundaries.

## Code Boundaries

### `src/core`

Responsibilities:

- message and tool types
- provider contract
- tool registry
- agent loop

Rules:

- no provider SDK imports
- no HTTP or Ink types

### `src/runtime`

Responsibilities:

- assemble the general tool registry
- load control files and config
- build the final system prompt
- run the runtime engine and runtime service
- own runtime/session store interfaces and implementations

Rules:

- keep orchestration here, not in HTTP
- keep session as context storage, not runtime policy

### `src/http`

Responsibilities:

- expose run, session, and event resources
- expose JSON and SSE event reads
- generate OpenAPI from route schemas
- project snapshots from the runtime store

Rules:

- keep the surface narrow
- keep handlers thin
- do not let request handlers become the source of truth for runtime state

### `src/tools`

Responsibilities:

- define and validate tool parameters
- expose the model-facing file-view and command tools
- call into filesystem or execution helpers

### `src/config`

Responsibilities:

- load workspace and user config
- resolve provider auth from environment variables

### `src/fs`

Responsibilities:

- define filesystem boundaries
- normalize and resolve paths
- traverse files
- perform text search

### `src/prompting`

Responsibilities:

- scan control documents
- assemble the runtime system prompt

### `src/providers`

Responsibilities:

- translate between core message/tool shapes and SDK-specific payloads
- stream provider deltas back into the core loop

### `src/clients`

Responsibilities:

- stay thin over the HTTP surface
- keep local UX concerns out of runtime code

## Runtime State

Runtime state lives behind explicit store boundaries rather than inside HTTP handlers.

Current shape:

- a file-backed runtime store owns sessions, runs, subscriptions, and append-only run events
- sessions store conversation history, run references, active-run state, and checkpoints
- the runtime engine owns run orchestration over the store, filesystem boundary, and execution backend
- tools can browse session history through a read-only `/session/...` file-view
- HTTP separately exposes session resource reads and run event streams

## Current Gaps

Known missing pieces:

- session history is still a dedicated read-only projection rather than a general writable mounted filesystem
- `cmd` still uses the local OS process backend by default
- session-wide event streaming does not exist yet

## Runtime Hands

The runtime depends on two explicit host boundaries:

- a filesystem boundary for file-backed behavior
- an execution backend for command execution

Local filesystem and local process execution are only the default implementations.
Future remote sandboxes should replace those boundaries, not rewrite HTTP or `src/core`.

## Dependency Rules

- `core` must stay independent of provider SDKs, HTTP, and Ink
- `providers` may depend on `core`, but not vice versa
- `tools` may depend on `core` and `fs`
- `runtime` may depend on `core`, `config`, `providers`, and `tools`
- `http` may depend on `core`, `runtime`, `tools`, `fs`, and `prompting`
- `clients` should depend on HTTP behavior and local client concerns only

## Product Rule

Default sequence for changes:

1. `src/core`
2. transport adapter updates only if needed
3. thin client updates only if needed

If a feature only exists to make the TUI nicer, that is usually not a good enough reason to reshape the system.
