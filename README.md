# picoagent

Minimal coding agent built around ACP and a local Ink TUI.

## What This Is

`picoagent` is a small TypeScript harness for running one coding agent per session:
- ACP agent over stdio
- local Ink client as the default UI
- one provider contract
- one tool-calling loop
- one tool registry with mode-based tool subsets

The current design is intentionally simple:
- no frontend-agent / backend-agent split
- no worker orchestration
- no HTTP control plane

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

The important detail is architectural, not just UX:
- all tools exist in one registry
- modes only decide which tools are equipped for the session

## Current Layout

```text
src/
  acp/        ACP agent entrypoint and ACP-backed environment
  app/        bootstrap for config, provider, and tool registry
  core/       loop, provider contract, tool registry, shared types
  providers/  Anthropic, OpenAI-compatible, Gemini adapters
  tools/      list/read/search/write/run-command tools
  tui/        local Ink ACP client
  lib/        config, prompt, frontmatter, filesystem helpers

defaults/
  skills/
  agents/

docs/
  INDEX.md
  architecture.md
  golden-principles.md
  runtime-model.md
  entrypoints.md
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

Workspace config overrides user config. If neither file exists, pico falls back to the built-in `echo` provider.

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

For local TUI smoke-testing without a real model, use:

```jsonc
{
  "provider": "echo",
  "model": "echo"
}
```

The built-in `echo` provider streams back `received: <your prompt>` and does not require an API key.

Run the local TUI:

```bash
OPENAI_API_KEY=... npm run dev
```

With `provider: echo`, or with no config file at all, you can just run:

```bash
npm run dev
```

Run only the ACP agent:

```bash
OPENAI_API_KEY=... npm run dev:agent
```

## Documentation

Start here:
- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/runtime-model.md`
- `docs/entrypoints.md`

## License

MIT
