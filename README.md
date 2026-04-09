# picoagent

Minimal coding agent built around ACP. The ACP stdio server is the product surface; the Ink TUI is a thin local debug client.

## What This Is

`picoagent` is a small TypeScript agent stack with one core loop and one ACP transport:

- ACP server over stdio
- one provider contract
- one tool-calling loop
- one global tool registry with mode-based tool subsets
- optional local Ink client for development smoke tests

The project is intentionally narrow:

- no multi-agent orchestration
- no HTTP control plane
- no UI-owned model logic
- no requirement that Pico ship its own primary client

## Product Stance

The durable asset in this repo is `core + ACP server`.

That means:

- new behavior should land in `src/core` or `src/acp` first
- the TUI exists to inspect and debug the server locally
- terminal UX work should stay minimal unless it directly supports server verification

## Modes

`picoagent` exposes two session modes:

- `ask`
  - inspect files
  - list files
  - search text
  - explain and plan
- `exec`
  - everything in `ask`
  - write files
  - run commands

Modes do not create separate runtimes. They only choose which tools the session equips.

## Layout

```text
src/
  acp/        ACP stdio server, session lifecycle, transport adapter
  bootstrap/  runtime assembly for config, provider, and tool registry
  clients/    thin replaceable clients
    tui/      local Ink debug client for the ACP server
  core/       loop, provider contract, tool registry, shared types
  config/     config loading and provider env resolution
  fs/         deterministic filesystem traversal and search helpers
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

### Run The ACP Server

Development:

```bash
npm run dev
```

Built output:

```bash
npm run build
npm run start
```

### Run The Local TUI

The TUI is a local development client, not the primary product surface.

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
