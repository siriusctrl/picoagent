# Architecture

## Current Scope

The repository is intentionally a single TypeScript package.

That is a deliberate choice, not an incomplete monorepo.
The current boundaries are clearer as directories than as publishable packages.

## Code Boundaries

### `src/core`

The kernel.

Responsibilities:
- message and tool types
- provider interface
- lifecycle hooks contract
- the tool-calling agent loop

Rules:
- no provider SDK imports
- no entrypoint IO concerns
- no workspace-specific policy beyond the generic tool context

### `src/runtime`

Orchestration around the kernel.

Responsibilities:
- main-agent session state
- worker lifecycle
- completion notifications back into the main session
- worker control hooks and control state

### `src/hooks`

Composable lifecycle adapters.

Responsibilities:
- tracing
- compaction
- worker-control integration points

These are optional extensions to the loop, not alternate runtimes.

### `src/providers`

SDK adapters only.

Responsibilities:
- translate picoagent message/tool shapes to provider-specific SDK calls
- parse provider responses back into picoagent message/tool shapes

### `src/tools`

LLM-facing capabilities.

Responsibilities:
- define tool parameters
- enforce boundary validation for tool args
- call into lower-level helpers

### `src/lib`

Shared filesystem and contract helpers.

Responsibilities:
- prompt assembly
- frontmatter parsing and scanning
- config loading
- task file management
- workspace creation
- sandbox execution
- git helpers

### `src/app`

Entrypoint assembly.

Responsibilities:
- load config from the control workspace
- assemble tool sets
- create the runtime
- connect runtime callbacks to REPL or server presentation

## Dependency Rules

- `core` must stay independent of provider SDKs and entrypoint code.
- `runtime` may depend on `core`, `hooks`, and `lib`.
- `providers` may depend on `core`, but `core` must not depend on `providers`.
- `tools` may depend on `core` and `lib`.
- `app` may depend on everything else.

## Why One Package

The repo does not yet have package-level dependency pressure strong enough to justify a monorepo split.

Today, a single package is the stronger design because it keeps:
- navigation simple
- refactors cheap
- type boundaries local
- contributor overhead low

If a future split happens, it should come from operational need:
- separate distribution targets
- materially different release cadence
- conflicting dependency surfaces

Not from aesthetics.
