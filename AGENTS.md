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

Keep this file coarse-grained. Do not try to mirror every subdirectory here. For source-area guidance, read `docs/source-map.md`.

### Read these docs first

- `README.md`
- `docs/architecture.md`
- `docs/design-choices.md`
- `docs/golden-principles.md`

### Read these docs when the task matches

- Session behavior, agent behavior, or tool access:
  - Read `docs/runtime-model.md`
- HTTP or local UI entrypoints:
  - Read `docs/entrypoints.md`
- Architecture or boundary changes:
  - Read `docs/architecture.md`
  - Read `docs/design-choices.md`

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
- When a design discussion converges on a durable choice, record it promptly in `docs/design-choices.md`.
- Prefer keeping detailed source-area guidance in `docs/source-map.md`, not local `AGENTS.md` files under `src/`.

## Verification Requirements

- Minimum bar for meaningful changes:
  - `bun run build`
  - `bun run test`
  - `bun run typecheck`
- If you change TUI behavior, manually run `bun run dev:tui`.

## Collaboration Preferences

- Keep implementations small and legible.
- Optimize for code that future agents can read in one pass.
- Hold the architectural bar requested by the user; do not quietly retreat to a weaker design for convenience.
- When a large refactor can be split into clear, low-conflict chunks, prefer using multiple agents.
- Keep one main agent responsible for decomposition, integration, verification, and final quality decisions.

## Agent Strategy Defaults

Use these as workflow defaults for large refactors, not as a reason to introduce multi-agent runtime architecture into the product.

- Prefer forking subagents only when ownership can be split cleanly by module, boundary, or file set.
- Do not delegate the immediate critical-path blocker if the main agent needs that answer before it can proceed.
- Give each subagent an explicit, bounded write scope and a concrete outcome to return.
- Avoid parallel agent work when tasks are tightly coupled or likely to cause overlapping edits.
- For explicit, bounded coding tasks, prefer Codex subagents running `gpt-5.3-codex-spark`.
- For architectural judgment, tricky debugging, broad reasoning, or ambiguous review, prefer Codex subagents running `gpt-5.4` with `high` reasoning.
- Keep the main agent responsible for architecture decisions, merge-quality judgment, end-to-end verification, and final integration.
