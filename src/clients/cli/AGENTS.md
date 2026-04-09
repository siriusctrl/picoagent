Local guidance for `src/clients/cli`.

## Scope

`src/clients/cli` is the minimum command-line client for local Pico entrypoints.

## Rules

- Keep the CLI thin and explicit.
- Reuse the HTTP surface instead of inventing a second runtime path.
- Prefer a small command surface over feature-heavy shell UX.
- If CLI behavior changes, verify with `npm run dev:cli -- help`, `serve`, and at least one `run` command.

## Read First

- `docs/entrypoints.md`
- `src/clients/cli/main.ts`
- `src/http/server.ts`
