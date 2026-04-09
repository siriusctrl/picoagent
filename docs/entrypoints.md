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

HTTP resource model:

- `session` is the context container
- `session` carries a default agent preset
- `run` is one execution
- session runs inherit the session default agent unless the request overrides it
- `events` are the ordered records for one run
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
- `run` starts a local HTTP run, streams its events, and exits

This client is also thin. It should prefer reusing the existing runtime paths over inventing a second agent architecture.

## Shared Bootstrap

All entrypoints rely on the same bootstrap path:

1. load config from the control workspace
2. create the provider
3. assemble the global tool registry
4. build the system prompt for the active agent preset
5. create transport-specific run and session state only where needed

The boundary is:

- bootstrap defines the agent shape
- HTTP defines protocol details
- clients stay replaceable
