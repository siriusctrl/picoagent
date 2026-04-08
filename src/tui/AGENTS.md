Local guidance for `src/tui`.

## Scope

`src/tui` is the local terminal client for the ACP agent.

## Rules

- Keep the TUI thin; it should speak ACP and render terminal UX, not own model logic.
- Do not couple this directory to provider internals.
- Preserve terminal-native behavior over browser-style patterns.
- If TUI behavior changes, verify with `npm run dev`.

## Read First

- `docs/entrypoints.md`
- `src/tui/controller.ts`
- `src/tui/main.tsx`
