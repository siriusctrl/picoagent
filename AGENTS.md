Principles for agents contributing to this repository.

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

4. **Keep transports thin**
   - Treat `src/core` as the runtime boundary.
   - Keep `src/core` transport-agnostic.
   - Keep HTTP-specific behavior in `src/http` and UI-specific behavior in `src/clients`.

5. **Conventional Commits with real bodies**
   - Use Conventional Commits for every commit.
   - Do not write title-only commits.
   - In the commit body, explain both:
     - what changed
     - why the change was made

## Navigation

Use docs to understand constraints first. Use source directories only after you know which boundary matters.

Keep this file coarse-grained. Do not try to mirror every subdirectory here. For source-area guidance, read `src/AGENTS.md` and `docs/source-map.md`.

### Read these docs first

- `README.md`
- `docs/architecture.md`
- `docs/golden-principles.md`

### Read these docs when the task matches

- Session behavior, agent behavior, or tool access:
  - Read `docs/runtime-model.md`
- HTTP or local UI entrypoints:
  - Read `docs/entrypoints.md`
- Architecture or boundary changes:
  - Read `docs/architecture.md`

## Top-level Source Map

- `src/core` - loop, provider contract, tool registry, shared types
- `src/http` - async HTTP server for runs, sessions, and events
- `src/clients` - local CLI and TUI clients
- `src/providers` - model SDK adapters
- `src/tools` - LLM-facing tools
- `src/config` - config loading and provider env resolution
- `src/fs` - filesystem traversal and search helpers
- `src/prompting` - prompt framing and frontmatter scanning
- `src/runtime` - runtime context assembly and session control snapshots
- `tests` - focused contract tests

## Task Routing

- For loop, tool, or provider-contract work, inspect `src/core` first.
- For HTTP behavior, inspect `src/http`.
- For terminal UI changes, inspect `src/clients/tui`.
- For tool behavior, inspect `src/tools`.
- For shared helpers, inspect `src/config`, `src/fs`, and `src/prompting`.
- For agent wiring or runtime context assembly, inspect `src/runtime/index.ts` and `src/core/tool-registry.ts`.

## Engineering Rules

- TypeScript everywhere.
- Validate external, model, and tool input at boundaries.
- Keep provider SDK imports out of `src/core`.
- Keep tools focused: one tool, one capability.
- Update docs when architecture, runtime behavior, or entrypoints change.
- Prefer keeping detailed source-area guidance in `src/AGENTS.md` and `docs/source-map.md`.

## Verification Requirements

- Minimum bar for meaningful changes:
  - `npm run build`
  - `npm test`
  - `npm run typecheck`
- If you change TUI behavior, manually run `npm run dev:tui`.

## Collaboration Preferences

- Keep implementations small and legible.
- Optimize for code that future agents can read in one pass.
- Hold the architectural bar requested by the user; do not quietly retreat to a weaker design for convenience.
