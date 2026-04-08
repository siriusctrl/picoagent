Principles for agents contributing to this repository.

## Mission

Build a minimal agent framework that stays understandable from the repository alone.

The project has two equal goals:
1. Be useful as a real coding and orchestration tool.
2. Stay small enough that humans and agents can audit the whole system without framework archaeology.

## Core Principles

1. **Kernel first**
   - Keep the agent loop, provider contract, and tool contract small and explicit.
   - Protect `src/core/` from SDK leakage and orchestration sprawl.

2. **Control workspace is the source of truth**
   - Prompts, skills, memory, and agent instructions come from the user-managed control directory.
   - Execution workspaces exist to run tasks safely, not to redefine project intent.

3. **Use the simplest boundary that preserves clarity**
   - Prefer one package with clear directories over a monorepo with ceremonial package splits.
   - Add a new package only when dependency boundaries become operationally useful, not aesthetically pleasing.

4. **No speculative platform layers**
   - Do not add plugin buses, registries, transport layers, persistence systems, or compatibility shims unless the task explicitly requires them.
   - Keep the current REPL and HTTP surfaces honest about what they support today.

## Navigation

Start with docs, then inspect code once you know which boundary you are changing.

### Read these docs first

- `README.md` - project overview, entrypoints, and development commands.
- `docs/INDEX.md` - docs map and reading order.
- `docs/architecture.md` - code boundaries and dependency rules.
- `docs/golden-principles.md` - durable invariants.

### Read these docs when the task matches

- Runtime, task, worker, or workspace changes:
  - Read `docs/runtime-model.md` first.
- REPL or HTTP entrypoint changes:
  - Read `docs/entrypoints.md` first.
- Architecture or directory-boundary changes:
  - Read `docs/architecture.md` first.

## Top-level Source Map

- `src/core` - agent loop, hook contract, provider contract, shared message/tool types.
- `src/runtime` - runtime orchestration, worker lifecycle, worker control.
- `src/hooks` - composable lifecycle adapters such as tracing and compaction.
- `src/providers` - SDK-specific provider adapters.
- `src/tools` - tool definitions exposed to the LLM.
- `src/lib` - filesystem, prompt, task, sandbox, and git helpers.
- `src/app` - entrypoint bootstrap and runtime assembly.
- `defaults` - built-in skill and agent metadata shipped with picoagent.
- `tests` - mirrors the source layout where practical.

## Task Routing

When starting work, route yourself by task type:

- For an unfamiliar task, read `README.md`, then `docs/INDEX.md`, then only the matching docs.
- For loop/provider/tool contract changes, inspect `src/core` before touching providers or tools.
- For worker dispatch, sandbox, worktree, or task lifecycle changes, inspect `src/runtime`, `src/lib/task.ts`, `src/lib/workspace.ts`, and `src/lib/sandbox.ts`.
- For prompt, skills, or instruction-loading changes, inspect `src/lib/prompt.ts` and `defaults/`.
- For REPL or HTTP changes, inspect `src/app`, `src/main.ts`, and `src/server.ts`.

## Engineering Rules

- TypeScript everywhere.
- Keep `src/core` free of provider SDK imports.
- Validate external inputs at trust boundaries, not everywhere.
- Keep tools focused: one tool, one capability.
- Worker write access must stay constrained to the task workspace.
- Keep runtime IO concerns out of core logic when a callback boundary is sufficient.
- Update docs when runtime contracts, architecture, or entrypoint behavior changes.

## Verification Requirements

- Minimum bar for meaningful code changes:
  - `npm run build`
  - `npm test`
  - `npm run typecheck`
- If you change runtime/workspace/task behavior, add or update deterministic tests for the changed contract.
- If you change REPL or server entrypoints, run the relevant entrypoint manually after build:
  - `npm run dev`
  - `npm run dev:server`
- If a feature depends on Linux-only sandboxing behavior, tests must skip or degrade cleanly when the environment blocks it.

## Collaboration Preferences

- Keep implementations small and legible.
- Prefer explicit contracts over clever indirection.
- Optimize for future readers who need to understand the system in one sitting.
- If the requested goal implies a real architecture change, implement the coherent end state directly instead of layering temporary shims on top of confused boundaries.
