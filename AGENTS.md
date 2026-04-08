Principles for agents contributing to this repository.

## Mission

Build a minimal coding agent that is:
1. genuinely usable from a terminal
2. small enough to audit in one sitting

## Core Principles

1. **KISS**
   - Prefer the simplest solution that works.
   - Avoid premature abstraction.
   - If a function is enough, do not create a class.
   - If one clear module is enough, do not invent a framework.

2. **No speculative forward-compatibility**
   - Do not add extension points, plugin systems, compatibility shims, or worker orchestration unless explicitly requested.
   - Do not reintroduce frontend-agent / backend-agent splitting.
   - Build for the current design, not hypothetical future variants.

3. **Prefer strongest end-state solutions**
   - When the right design is clear and requested, implement the coherent end state directly.
   - Do not intentionally land transitional layers or partial architectures as stepping stones.

4. **ACP is the transport contract**
   - Treat stdio ACP as the agent/client boundary.
   - Keep `src/core` transport-agnostic.
   - Keep ACP-specific behavior in `src/acp` and UI-specific behavior in `src/tui`.

5. **Conventional Commits with real bodies**
   - Use Conventional Commits for every commit.
   - Do not write title-only commits.
   - In the commit body, explain both:
     - what changed
     - why the change was made

## Navigation

Use docs to understand constraints first. Use source directories only after you know which boundary matters.

Keep this file coarse-grained. Do not try to mirror every subdirectory here. For detailed structure or local rules inside a source area, inspect that directory's local `AGENTS.md` when it exists.

### Read these docs first

- `README.md`
- `docs/architecture.md`
- `docs/golden-principles.md`

### Read these docs when the task matches

- Session behavior, mode behavior, or tool access:
  - Read `docs/runtime-model.md`
- ACP or local UI entrypoints:
  - Read `docs/entrypoints.md`
- Architecture or boundary changes:
  - Read `docs/architecture.md`

## Top-level Source Map

- `src/core` - loop, provider contract, tool registry, shared types
- `src/acp` - ACP agent entrypoint and ACP-backed environment
- `src/tui` - local Ink ACP client
- `src/providers` - model SDK adapters
- `src/tools` - LLM-facing tools
- `src/lib` - filesystem, prompt, config, and shared helpers
- `src/app` - bootstrap and registry assembly
- `tests` - focused contract tests

## Task Routing

- For loop, tool, or provider-contract work, inspect `src/core` first.
- For ACP behavior, inspect `src/acp`.
- For terminal UI changes, inspect `src/tui`.
- For tool behavior, inspect `src/tools`.
- For shared helpers, inspect `src/lib`.
- For mode wiring or registry assembly, inspect `src/app/bootstrap.ts` and `src/core/tool-registry.ts`.

## Engineering Rules

- TypeScript everywhere.
- Validate external, model, and tool input at boundaries.
- Keep provider SDK imports out of `src/core`.
- Keep tools focused: one tool, one capability.
- Update docs when architecture, runtime behavior, or entrypoints change.
- Prefer moving detailed local guidance into directory-level `AGENTS.md` files instead of growing this file.

## Verification Requirements

- Minimum bar for meaningful changes:
  - `npm run build`
  - `npm test`
  - `npm run typecheck`
- If you change TUI behavior, manually run `npm run dev`.
- If you change ACP session behavior or tool contracts, add or update deterministic tests.

## Collaboration Preferences

- Keep implementations small and legible.
- Optimize for code that future agents can read in one pass.
- Hold the architectural bar requested by the user; do not quietly retreat to a weaker design for convenience.
