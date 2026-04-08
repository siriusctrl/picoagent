Principles for agents contributing to this repository.

## Mission

Build a minimal coding agent that is:
1. genuinely usable from a terminal
2. small enough to audit in one sitting

## Core Principles

1. **Single-session agent**
   - Keep one agent loop per ACP session.
   - Do not reintroduce frontend-agent / backend-agent role splitting.
   - Do not reintroduce worker orchestration unless explicitly requested.

2. **Mode gates tools, not architecture**
   - Keep one general tool registry.
   - `ask` and `exec` decide which tools are equipped for the session.
   - Do not scatter mode checks across unrelated code when the registry boundary is enough.

3. **ACP is the transport contract**
   - Treat stdio ACP as the agent/client boundary.
   - Keep `src/core` transport-agnostic.
   - Keep ACP-specific adaptation in `src/acp` and UI-specific behavior in `src/tui`.

4. **Keep it legible**
   - Prefer plain functions and small modules over framework-looking layers.
   - Build the strongest clear end state directly instead of layering compatibility shims.

## Navigation

Start with docs, then inspect code once you know which boundary matters.

### Read these docs first

- `README.md`
- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/golden-principles.md`

### Read these docs when the task matches

- Session behavior, mode behavior, or tool access:
  - read `docs/runtime-model.md`
- ACP or local UI entrypoints:
  - read `docs/entrypoints.md`
- directory-boundary or responsibility changes:
  - read `docs/architecture.md`

## Top-level Source Map

- `src/core` - provider contract, loop, tool registry, shared types
- `src/acp` - ACP agent entrypoint and ACP-backed environment
- `src/tui` - Ink client and local terminal UX
- `src/providers` - model SDK adapters
- `src/tools` - LLM-facing tools
- `src/lib` - config, prompt, frontmatter, filesystem helpers
- `src/app` - bootstrap for provider plus registry assembly
- `defaults` - built-in skill and agent metadata
- `tests` - focused contract tests

## Task Routing

- For loop, tool, or provider-contract work, inspect `src/core` first.
- For mode changes or tool availability changes, inspect `src/app/bootstrap.ts` and `src/core/tool-registry.ts`.
- For ACP behavior, inspect `src/acp`.
- For terminal UI changes, inspect `src/tui`.
- For prompt framing, inspect `src/lib/prompt.ts` and `defaults/`.

## Engineering Rules

- TypeScript everywhere.
- Keep provider SDK imports out of `src/core`.
- Validate model/tool input at the boundary.
- Keep tools focused: one tool, one capability.
- Update docs when architecture, mode behavior, or entrypoint behavior changes.
- Prefer changing the registry or prompt contract over adding special cases.

## Verification Requirements

- Minimum bar for meaningful changes:
  - `npm run build`
  - `npm test`
  - `npm run typecheck`
- If you change TUI behavior, manually run `npm run dev`.
- If you change ACP session behavior or tool contracts, add or update deterministic tests.

## Collaboration Preferences

- Keep implementations small and explicit.
- Optimize for future agents reading the code in one pass.
- If the requested target is architectural, implement the coherent end state directly.
