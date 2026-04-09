Local guidance for `src/core`.

## Scope

`src/core` is the kernel:
- agent loop
- provider contract
- tool registry
- shared message and tool types

## Rules

- Keep this directory transport-agnostic.
- Keep HTTP, Ink, and provider SDK details out of here.
- Validate external and model-produced inputs before core logic assumes shape.
- Prefer plain functions and small modules over framework-looking layers.
- If behavior here changes outward contracts, update the relevant deterministic tests.

## Read First

- `docs/architecture.md`
- `docs/runtime-model.md`
- `src/core/loop.ts`
- `src/core/types.ts`
- `src/core/tool-registry.ts`
