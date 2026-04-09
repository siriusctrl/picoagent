# Entrypoints

## HTTP Server

Primary entry file: `src/http/server.ts`

Behavior:

- serves a minimal local HTTP API
- reuses the shared bootstrap path and core loop
- exposes async-first run, session, and event resources
- keeps streaming and non-streaming reads behind the same `/events/:runId` endpoint

Current endpoints:

- `GET /openapi.json`
- `POST /runs`
- `GET /runs/:id`
- `GET /events/:runId`
- `POST /sessions`
- `GET /sessions/:id`
- `POST /sessions/:id/agent`
- `POST /sessions/:id/runs`
- `POST /sessions/:id/compact`

HTTP resource model:

- `session` is the context container
- `session` binds one workspace root
- `session` carries a default agent preset
- `session` caches a control snapshot derived from workspace control files
- `session` may also carry a checkpointed summary of older history
- `run` is one execution
- session runs inherit the session default agent unless the request overrides it
- session runs refresh the cached control snapshot automatically if the workspace changed
- `events` are the ordered records for one run
- compaction creates a checkpoint plus recent tail without deleting run or event history
- set `Accept: text/event-stream` on `GET /events/:runId` for streaming
- omit that header to read the same event log as JSON

This is the main product entrypoint.

## Local TUI

Secondary entry file: `src/clients/tui/main.tsx`

Behavior:

- starts a local Ink UI
- starts a local HTTP-backed session rooted at the current working directory
- renders streamed assistant output and tool activity
- lets the user switch between the built-in agent presets

This client is for local development and debugging. It should stay thin.

## CLI

Secondary entry file: `src/clients/cli/main.ts`

Behavior:

- exposes a minimum command surface
- `serve` starts the HTTP server in the foreground

This client is also thin. It should prefer reusing the existing runtime paths over inventing a second agent architecture.

## Shared Bootstrap

All entrypoints rely on the same bootstrap path:

1. resolve the workspace root
2. assemble the global tool registry
3. build or refresh the session control snapshot when a session needs it
4. create the provider for the current control snapshot
5. create transport-specific run and session state only where needed

The boundary is:

- bootstrap defines the agent shape
- HTTP defines protocol details
- clients stay replaceable
