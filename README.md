# picoagent

Minimal coding agent with one core runtime and one async HTTP server.

## What This Is

`picoagent` is a small TypeScript agent stack with one core loop and one thin transport adapter:

- minimal HTTP server for local automation and scripting
- one provider contract
- one tool-calling loop
- one global tool registry with agent-based tool subsets
- optional local Ink client for development smoke tests

The project is intentionally narrow:

- no multi-agent orchestration
- no UI-owned model logic
- no requirement that Pico ship its own primary client

## Product Stance

The durable asset in this repo is `core + HTTP`.

That means:

- new behavior should land in `src/core` first
- HTTP is the main product surface
- the TUI exists as a thin local HTTP client
- terminal UX work should stay minimal unless it directly supports runtime verification

## Agents

`picoagent` exposes two built-in agent presets:

- `ask`
  - inspect files
  - list files
  - search text
  - explain and plan
- `exec`
  - everything in `ask`
  - write files
  - run commands

Agent presets do not create separate runtimes. They only choose which tools the run equips.

## Layout

```text
src/
  bootstrap/  runtime assembly for config, provider, and tool registry
  clients/    thin replaceable clients
    cli/      minimum command-line client and local entrypoint
    tui/      local Ink smoke-test client for the HTTP server
  core/       loop, provider contract, tool registry, shared types
  config/     config loading and provider env resolution
  fs/         deterministic filesystem traversal and search helpers
  http/       minimal async HTTP adapter for runs, sessions, and events
  prompting/  prompt assembly and frontmatter-backed prompt scanning
  providers/  Anthropic, OpenAI-compatible, Gemini adapters
  tools/      list/read/search/write/run-command tools
```

## Development

```bash
npm install
npm run build
npm test
npm run typecheck
```

## Usage

Pico looks for config in:

- `./.pico/config.jsonc`
- `~/.pico/config.jsonc`

Workspace config overrides user config. If neither file exists, Pico falls back to the built-in `echo` provider.

Example workspace config:

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

### Run The HTTP Server

Development:

```bash
npm run dev
```

Built output:

```bash
npm run build
npm run start
```

Or explicitly:

Development:

```bash
npm run dev:http
```

Built output:

```bash
npm run build
npm run start:cli -- serve
```

Endpoints:

- `GET /openapi.json`
- `POST /runs`
- `GET /runs/:id`
- `GET /events/:runId`
- `POST /sessions`
- `GET /sessions/:id`
- `POST /sessions/:id/agent`
- `POST /sessions/:id/runs`

HTTP is async-first:

- `POST /runs` and `POST /sessions/:id/runs` start work and immediately return a `runId`
- `GET /runs/:id` returns the current run snapshot
- `GET /events/:runId` returns the run event log as JSON
- set `Accept: text/event-stream` on `GET /events/:runId` to stream the same events over SSE

Sessions are context containers bound to one workspace root. Each session has a default agent preset plus a cached control snapshot resolved from workspace control files like `AGENTS.md`, `USER.md`, `SOUL.md`, and `.pico/config.jsonc`. Session runs inherit the default agent unless the run request overrides it with its own `agent`.

Before starting a session run, the server checks whether the bound workspace changed. If it did, the session control snapshot is refreshed automatically before the run starts.

### Run The CLI

Built output:

```bash
npm run build
npm run start:cli -- help
npm run start:cli -- serve
```

Development:

```bash
npm run dev:cli -- help
npm run dev:cli -- serve
```

### Run The Local TUI

The TUI is the local interactive client for the HTTP server, not the primary product surface.

With a real provider:

```bash
OPENAI_API_KEY=... npm run dev:tui
```

For local smoke tests with the built-in echo provider:

```bash
npm run dev:tui
```

Built output:

```bash
npm run build
npm run start:tui
```

The built-in `echo` provider streams back `received: <your prompt>` and does not require an API key.

## Documentation

Start here:

- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/runtime-model.md`
- `docs/entrypoints.md`

## License

MIT
