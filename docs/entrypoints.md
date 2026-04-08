# Entrypoints

## REPL

Entry file: `src/main.ts`

Behavior:
- loads config from the control workspace
- assembles the runtime once
- streams assistant text to stdout
- routes background worker completions back into the main session

The REPL prints:
- control workspace path
- execution repo path
- execution mode
- task workspace root

That output is intentional. It makes the runtime topology visible up front.

## HTTP Server

Entry file: `src/server.ts`

Behavior:
- loads the same runtime bootstrap as the REPL
- exposes chat over SSE
- exposes task inspection and worker control endpoints

Endpoints:
- `POST /chat`
- `GET /tasks`
- `GET /tasks/:id`
- `POST /tasks/:id/steer`
- `POST /tasks/:id/abort`

## Process Scope

Task state exposed by the HTTP server is scoped to the runtime created by that server process.

That means:
- restarting the server creates a new run workspace
- `/tasks` lists tasks for the current process-run

This is the honest contract for the current implementation.
It is not a persistence layer.

## Runtime Assembly

Both entrypoints use the same bootstrap path:

1. load config from the control workspace
2. resolve the execution repo and task workspace root
3. build prompt context from the control workspace
4. create provider, tools, and runtime
5. connect runtime callbacks to presentation-specific IO

The important boundary is that presentation stays in the entrypoint, while runtime orchestration stays in `src/runtime`.
