# picoagent

Simple, controllable agent harness with one HTTP API.

## What It Is

`picoagent` is a small TypeScript codebase for building a controllable agent harness.

The repository is Bun-first for local development, testing, and CI.
The default local runtime is Bun-native end to end: HTTP serving, filesystem access, hashing, globbing, process execution, and tests all run through Bun APIs unless a boundary absolutely requires something else.
Source files use native `.ts` and `.tsx` import specifiers, and the TypeScript build rewrites them to `.js` in `dist/`.
Those Bun-specific choices stay behind the existing runtime boundaries so `session`, `runtime`, `filesystem`, and `execution backend` semantics do not depend on one transport or one host API surface.

The core design goal is to keep four concerns explicit and separate:

- context management through sessions
- execution through the runtime engine
- file access through filesystem-like boundaries and tools
- command execution through an execution backend

For durable architecture choices and intentionally rejected alternatives, read `docs/design-choices.md`.

The current reference shape is:

- one core agent loop
- one global tool registry
- one async Bun-hosted Hono HTTP runtime surface
- thin local clients for debugging

The current implementation stays intentionally small:

- no UI-owned model logic
- no transport-specific runtimes
- no orchestration-first framework layers

This does not rule out higher-level orchestration later, including multi-agent setups.
The point is to keep the harness itself legible and fully controllable first.

What is already present:

- explicit session objects for persistent context management
- one shared runtime loop model for runs
- file-backed runtime state under `.pico/runtime/`
- a model-facing session file-view for browsing compacted history without forcing it all into the live prompt
- a runtime engine that owns prompt assembly, control refresh, tool wiring, and run orchestration
- standalone filespace and session services that can be started before the runtime binds to them

What is still incomplete:

- session history is still exposed as a dedicated read-only projection rather than a general mounted filesystem namespace
- `cmd` still uses the local process backend by default
- event streaming exists per run, but not yet for the whole session

## Quick Start

Install and run the local server:

```bash
bun install
bun run dev
```

That starts the HTTP server from the current working directory.
Persistent runtime state is stored under `.pico/runtime/`.

Build output:

```bash
bun run build
```

The default `start` scripts also run source files directly through Bun:

```bash
bun run start
```

Verification:

```bash
bun run build
bun run test
bun run typecheck
```

## Config

Pico builds an effective config from up to two files:

- `<workspace>/.pico/config.jsonc`
- `$HOME/.pico/config.jsonc`

The merge is shallow:

- user config provides defaults
- workspace config overrides only the fields it sets
- built-in defaults fill anything still missing

Pico does not search parent directories. If neither file exists, it falls back to the built-in `echo` provider.

Example:

```jsonc
{
  "provider": "openai",
  "model": "gpt-4o"
}
```

Supported providers:

- `anthropic`
- `openai`
- `gemini`
- `echo`

Environment variables:

- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `GEMINI_API_KEY`

The built-in `echo` provider streams back `received: <your prompt>` and does not require an API key.

## Runtime Model

The important split is:

- `session` stores context
- `runtime` reads control files, assembles prompts, and executes runs
- `filesystem` provides workspace and other file-backed inputs
- `execution backend` runs commands and other executable work

HTTP is the main surface that ties those pieces together without collapsing them into one module.
For model-side history lookup, a session also exposes a read-only file-view built from summaries, checkpoints, and past runs.

The runtime exposes one tool surface:

- `glob`
- `grep`
- `read`
- `patch`
- `cmd`

The model is controlled through workspace and host control files, not through multiple runtime agent presets.

### Workspace

The current directory is the workspace. It is the main writable filesystem the runtime and tools operate on.

It contains both ordinary files and control files such as:

- `.pico/config.jsonc`
- `AGENTS.md`
- `USER.md`
- `SOUL.md`
- `.pico/memory/`
- `skills/`

### Session

A session is a persistent context log.

In the current implementation, a session is created against one workspace root. That binding is an implementation choice for the local harness, not the main semantic point of the abstraction.

Each session stores:

- conversation history
- optional checkpoints created by compaction
- ordered run ids and active-run state for projection purposes

The session does not assemble prompts and it does not carry runtime policy.
The runtime keeps a control snapshot cache, checks whether the relevant control files changed, and only rebuilds the prompt inputs when they do.

### Session History

Session compaction uses `checkpoint + tail`, not history deletion.

After compaction:

- older conversation turns move into a checkpoint summary
- recent messages stay active as the live tail
- full run and event history remains available to clients over HTTP

The model can browse session history through the session file-view with:

- `glob`
- `grep`
- `read`

That file-view is read-only. It is a projection of session state for model-side inspection, not a second writable workspace.
Raw event logs and compaction stay on the session or HTTP side, not the model tool surface.
Under Bun, `glob` matching follows Bun's glob syntax rather than a custom reduced parser.
For a workspace namespace with `cmd` enabled, `grep` prefers `rg` when available and falls back to the built-in file-view search when it is not.

File-view tools address mounted surfaces through paths such as:

- `/workspace/src/http/server.ts`
- `/session/summary.md`

Mounted file-views may also carry a small command capability flag:

- `supportsCmd`
  - whether `cmd` is enabled for that mounted view

`cmd` always requires an explicit namespace-rooted `cwd`.
The runtime appends a short file-view summary to the system prompt so the model sees the available surfaces and `cmd` policy directly.

## HTTP API

The server is async-first:

- `POST /runs` and `POST /sessions/:id/runs` return immediately with a `runId`
- `GET /runs/:id` returns the latest run snapshot
- `GET /events/:runId` returns the full event log as JSON
- `GET /events/:runId` with `Accept: text/event-stream` streams the same events over SSE

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

Create a session:

```bash
curl -X POST http://127.0.0.1:4096/sessions \
  -H 'content-type: application/json' \
  -d '{}'
```

For an explicitly isolated flow, start a session service first, create the session there, and then bind the runtime to it:

```bash
pico session serve --port 4097
pico serve --port 4096 --session http://127.0.0.1:4097
```

In that bound mode, clients still create sessions through the runtime. The difference is that the runtime persists those sessions in the external session service instead of a local in-process store.

Create a run in a session:

```bash
curl -X POST http://127.0.0.1:4096/sessions/<session_id>/runs \
  -H 'content-type: application/json' \
  -d '{"prompt":"summarize this repo"}'
```

Compact a session:

```bash
curl -X POST http://127.0.0.1:4096/sessions/<session_id>/compact \
  -H 'content-type: application/json' \
  -d '{"keepLastMessages":8}'
```

List session resources:

```bash
curl http://127.0.0.1:4096/sessions/<session_id>/resources
```

Read one session resource:

```bash
curl http://127.0.0.1:4096/sessions/<session_id>/resources/summary.md
```

OpenAPI is available at:

```text
GET /openapi
```

The OpenAPI document is generated from the HTTP route schemas rather than maintained as a separate hand-written spec.

## Local Clients

### CLI

The CLI is intentionally small. It starts the HTTP server and little else.

Default:

```bash
bun run start:cli -- help
bun run start:cli -- serve
```

Build output:

```bash
bun run build
```

### TUI

The TUI is a thin local HTTP client for smoke tests and debugging, not the primary product surface.

Default:

```bash
bun run start:tui
```

With a real provider:

```bash
OPENAI_API_KEY=... bun run start:tui
```

Build output:

```bash
bun run build
```

## Source Layout

```text
src/
  runtime/    runtime context assembly and runtime engine orchestration
  core/       loop, provider contract, tool registry, shared types
  http/       async HTTP server for runs, sessions, and events
  tools/      LLM-facing glob, grep, read, patch, and cmd tools
  providers/  Anthropic, OpenAI-compatible, Gemini, and echo adapters
  config/     config loading and provider env resolution
  fs/         filesystem traversal, path safety, and workspace FS boundary
  prompting/  prompt assembly and frontmatter scanning
  clients/    thin CLI and TUI clients
```

## Docs

Read in this order:

- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/golden-principles.md`
- `docs/runtime-model.md`
- `docs/entrypoints.md`
- `docs/source-map.md`

## License

MIT
