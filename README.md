# picoagent

Simple, controllable agent harness with one HTTP API.

## What It Is

`picoagent` is a small TypeScript codebase for building a controllable agent harness.

The core design goal is to keep three concerns explicit and separate:

- context management through sessions
- execution through the agent runtime
- file or history access through resources and tools

The current reference shape is:

- one core agent loop
- one global tool registry
- one async HTTP runtime surface
- thin local clients for debugging

The current implementation stays intentionally small:

- no UI-owned model logic
- no transport-specific runtimes
- no orchestration-first framework layers

This does not rule out higher-level orchestration later, including multi-agent setups.
The point is to keep the harness itself legible and fully controllable first.

What is already present:

- explicit session objects for context management
- one shared runtime loop model for runs
- workspace and session-history resources that tools can read without collapsing them into the live prompt

What is still incomplete:

- runtime state is still in-memory only
- session-history resources are not yet exposed as HTTP resources
- control snapshot reads still come from the local filesystem directly
- `run_command` still assumes a local OS process boundary

## Quick Start

Install and run the local server:

```bash
npm install
npm run dev
```

That starts the HTTP server from the current working directory.

Build and run the compiled output:

```bash
npm run build
npm run start
```

Verification:

```bash
npm run build
npm run test
npm run typecheck
```

## Config

Pico looks for config in:

- `./.pico/config.jsonc`
- `~/.pico/config.jsonc`

Workspace config overrides user config. If neither file exists, Pico falls back to the built-in `echo` provider.

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

- `session` manages context
- `runtime` executes runs
- `workspace` and session-history resources provide inputs the runtime can read

HTTP is the main surface that ties those pieces together without collapsing them into one module.

`picoagent` exposes two built-in agent presets:

- `ask`
  - list files
  - read files
  - search text
  - browse session history
- `exec`
  - everything in `ask`
  - compact session history
  - write files
  - run commands

Agent presets do not create separate runtimes. They only choose which tools a run equips.

### Workspace

The current directory is the workspace. It contains both ordinary files and control files such as:

- `.pico/config.jsonc`
- `AGENTS.md`
- `USER.md`
- `SOUL.md`
- `.pico/memory/`
- `skills/`
- `agents/`

### Session

A session is a persistent context container bound to one workspace root.

Each session stores:

- a default agent preset
- conversation history
- a cached control snapshot resolved from workspace control files
- optional checkpoints created by compaction

Before a session run starts, the server checks whether the workspace changed. If it did, the session control snapshot is refreshed automatically.

### Session History

Session compaction uses `checkpoint + tail`, not history deletion.

After compaction:

- older conversation turns move into a checkpoint summary
- recent messages stay active as the live tail
- full run and event history remains available as virtual session resources

The model can browse session history through:

- `list_session_resources`
- `read_session_resource`

The `exec` preset can also call `compact_session`.

## HTTP API

The server is async-first:

- `POST /runs` and `POST /sessions/:id/runs` return immediately with a `runId`
- `GET /runs/:id` returns the latest run snapshot
- `GET /events/:runId` returns the full event log as JSON
- `GET /events/:runId` with `Accept: text/event-stream` streams the same events over SSE

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

Create a session:

```bash
curl -X POST http://127.0.0.1:4096/sessions \
  -H 'content-type: application/json' \
  -d '{"agent":"ask"}'
```

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

OpenAPI is available at:

```text
GET /openapi.json
```

## Local Clients

### CLI

The CLI is intentionally small. It starts the HTTP server and little else.

Development:

```bash
npm run dev:cli -- help
npm run dev:cli -- serve
```

Built output:

```bash
npm run build
npm run start:cli -- help
npm run start:cli -- serve
```

### TUI

The TUI is a thin local HTTP client for smoke tests and debugging, not the primary product surface.

Development:

```bash
npm run dev:tui
```

With a real provider:

```bash
OPENAI_API_KEY=... npm run dev:tui
```

Built output:

```bash
npm run build
npm run start:tui
```

## Source Layout

```text
src/
  runtime/    runtime context assembly and session control snapshots
  core/       loop, provider contract, tool registry, shared types
  http/       async HTTP server for runs, sessions, and events
  tools/      LLM-facing file, session, write, and command tools
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
