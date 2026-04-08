# picoagent

Minimal agent orchestration for coding and task delegation.

## What this is

`picoagent` is a TypeScript agent framework built around a small kernel:
- one tool-calling loop
- one provider interface
- one runtime for main-agent and worker orchestration
- file-based prompts, skills, memory, tasks, and results

The project is intentionally not a platform. It is meant to stay understandable from the repository alone.

## Design Stance

- kernel first
- control workspace is the source of truth
- workers get isolated execution workspaces
- one package until package boundaries are operationally necessary
- explicit contracts over framework-looking abstraction

## Current Layout

```text
src/
  app/        runtime assembly and entrypoint bootstrap
  core/       agent loop, hooks contract, provider contract, shared types
  runtime/    main/worker orchestration
  hooks/      tracing and compaction adapters
  providers/  Anthropic, OpenAI-compatible, Gemini adapters
  tools/      shell, file, scan/load, dispatch/steer/abort
  lib/        prompts, tasks, workspace, sandbox, git, config helpers

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

## Runtime Model

`picoagent` distinguishes three things:

- Control workspace: the directory where you launch picoagent. It owns `config.md`, `AGENTS.md`, `SOUL.md`, `USER.md`, `memory/`, `skills/`, and `agents/`.
- Execution repo: the filesystem root where commands and edits run. If the control workspace is already inside a git repo, picoagent uses that repo directly. Otherwise it creates an isolated git snapshot for execution.
- Task workspaces: one directory per dispatched worker, usually created as git worktrees from the execution repo.

The important consequence is:
- prompts and skills come from the control workspace
- code execution happens in the execution repo or task workspaces
- worker writes stay bounded to the task workspace

Detailed behavior lives in `docs/runtime-model.md`.

## Development

```bash
npm install
npm run build
npm test
npm run typecheck
```

## Usage

Create a `config.md` in the control workspace:

```markdown
---
provider: openai
model: gpt-4o
---
```

Supported providers:
- `anthropic`
- `openai`
- `gemini`

API keys come from environment variables:
- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `GEMINI_API_KEY`

### REPL

```bash
OPENAI_API_KEY=... npm run dev
```

### HTTP Server

```bash
OPENAI_API_KEY=... npm run dev:server
```

Server endpoints:
- `POST /chat`
- `GET /tasks`
- `GET /tasks/:id`
- `POST /tasks/:id/steer`
- `POST /tasks/:id/abort`

## Documentation

Start here:
- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/runtime-model.md`
- `docs/entrypoints.md`

## License

MIT
