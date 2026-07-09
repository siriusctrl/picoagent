# AGENTS.md

This file is the operating map for agents working in this repo. Keep detailed
source-area guidance in `docs/source-map.md`, durable principles in
`docs/golden-principles.md`, and design rationale in `docs/design-choices.md`.

## Source Map

- `src/core/`: loop, execution, provider contract, tool registry, filesystem
  view, hooks, and shared types.
- `src/http/`: Bun/Hono HTTP server, sessions, filespaces, OpenAPI routes and
  schemas.
- `src/clients/cli/`: local CLI entrypoint and args.
- `src/clients/tui/`: terminal UI controller, input, layout, and history.
- `src/providers/`: model SDK adapters.
- `src/tools/`: model-facing tools.
- `src/config/`: config loading and provider env resolution.
- `src/fs/`: filesystem traversal, rooted/workspace/http filesystems, path and
  namespace helpers.
- `src/prompting/`: prompt framing and frontmatter scanning.
- `src/runtime/`: runtime context assembly, session stores, execution backend,
  and control snapshots.
- `docs/source-map.md`: detailed source-area routing.
- `docs/architecture.md`: runtime boundaries.
- `docs/runtime-model.md`: session, agent, and tool access behavior.
- `docs/entrypoints.md`: HTTP, CLI, and local UI entrypoints.

## Engineering Invariants

- TypeScript everywhere.
- Treat `src/core/` as the runtime boundary.
- Keep `src/core/` transport-agnostic.
- Keep HTTP-specific behavior in `src/http/` and UI-specific behavior in
  `src/clients/`.
- Keep provider SDK imports out of `src/core/`.
- Keep tools focused: one tool, one capability.
- Validate external, model, and tool input at boundaries.
- Do not reintroduce frontend-agent/backend-agent splitting.
- Do not add plugin systems, compatibility shims, worker orchestration, or
  speculative extension points unless explicitly requested.
- Prefer the strongest coherent end state over transitional layers when the
  requested design is clear.

## Task Routing

- Unknown task: read `README.md`, `docs/source-map.md`, and the matching doc
  below.
- Loop, tool, or provider-contract work: inspect `src/core/`.
- HTTP behavior: inspect `src/http/` and `docs/entrypoints.md`.
- Terminal UI: inspect `src/clients/tui/`.
- Tool behavior: inspect `src/tools/`.
- Runtime context or session control: inspect `src/runtime/` and
  `docs/runtime-model.md`.
- Architecture or boundary changes: read `docs/architecture.md` and
  `docs/design-choices.md`.

## Verification

- Run `bun run build`.
- Run `bun run test`.
- Run `bun run typecheck`.
- For TUI behavior, manually run `bun run dev:tui`.

## Docs Update Rules

- Runtime behavior, architecture, or entrypoints: update the matching file in
  `docs/`.
- Durable design choice: record it in `docs/design-choices.md`.
- Source-area routing changes: update `docs/source-map.md`.
- User-visible behavior or setup: update `README.md`.

## Agent Strategy Defaults

- Use subagents only when ownership can be split cleanly by module, boundary, or
  file set.
- Keep the main agent responsible for architecture decisions, integration,
  verification, and final merge quality.

## Commit Rules

- Use Conventional Commits with a body.
- Keep changes small and legible.
- Do not revert unrelated user changes.
