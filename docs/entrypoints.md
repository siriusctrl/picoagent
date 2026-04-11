# Entrypoints

Entrypoints should expose the harness boundaries, not redefine them.

In practice:

- `session` stays the context boundary
- `run` stays the execution boundary
- file-backed surfaces stay visible through explicit HTTP or tool-facing file-view paths

## HTTP Server

Primary entry file: `src/http/server.ts`

Behavior:

- serves the local Hono HTTP API hosted by Bun
- exposes async run, session, and event resources
- reuses the shared runtime context and core loop
- supports in-process app creation for tests and thin local clients
- generates `/openapi` from route schemas

Current endpoints:

- `GET /openapi`
- `POST /runs`
- `GET /runs/:id`
- `GET /events/:runId`
- `POST /sessions`
- `GET /sessions/:id`
- `GET /sessions/:id/resources`
- `GET /sessions/:id/resources/<resource_path>`
- `POST /sessions/:id/runs`
- `POST /sessions/:id/compact`

HTTP resource model:

- `session` is the context container
- `run` is one execution
- `events` are the ordered records for one run
- session history stays readable through session resource routes
- the model reads session history through the read-only `/session/...` file-view
- `GET /events/:runId` can return JSON or SSE depending on `Accept`

## Local TUI

Secondary entry file: `src/clients/tui/main.tsx`

Behavior:

- starts a local Ink UI
- starts a local HTTP-backed session rooted at the current working directory
- renders streamed assistant output and tool activity

This client is for local debugging only. It should stay thin.

## CLI

Secondary entry file: `src/clients/cli/main.ts`

Behavior:

- `pico serve` starts the runtime HTTP server
- `pico serve --mount <label=source> ...` adds extra file-view mounts before startup
- `pico serve --session <url>` binds the runtime to an external session service
- `pico session serve` starts the session service in the foreground
- `pico filespace serve` starts a rooted filespace HTTP service

Current status:

- `--mount` accepts local directory sources or remote filespace URLs
- mount labels must not reuse reserved names like `workspace` or `session`
- `--session` keeps session storage external while preserving the normal runtime-facing `POST /sessions` flow
- `session serve` exposes session creation, snapshots, compaction, and session resource reads without exposing run execution
- `filespace serve` prints a mountable endpoint for rooted file access

## Shared Runtime Path

All entrypoints rely on the same runtime path:

1. resolve the workspace root
2. assemble the general tool registry
3. load control files for the run
4. open runtime and session stores
5. create the provider
6. create transport-specific state only where needed

The boundary is:

- runtime context assembly defines the runtime behavior
- HTTP defines protocol details
- clients stay replaceable
