Local guidance for `src/clients/tui`.

## Scope

`src/clients/tui` is the local terminal debug client for the ACP agent.

## Rules

- Keep the TUI thin; it should speak ACP and render terminal UX, not own model logic.
- Treat this directory as a replaceable client, not the product center of gravity.
- Do not couple this directory to provider internals.
- Preserve terminal-native behavior over browser-style patterns.
- If TUI behavior changes, verify with `npm run dev:tui`.

## Read First

- `docs/entrypoints.md`
- `src/clients/tui/controller.ts`
- `src/clients/tui/main.tsx`
