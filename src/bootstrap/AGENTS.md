Local guidance for `src/bootstrap`.

## Scope

`src/bootstrap` assembles runtime dependencies.

## Rules

- Assemble config, provider, and tool registry here.
- Keep business logic out of bootstrap code.
- Prefer changing registry assembly over adding mode-specific special cases elsewhere.

## Read First

- `docs/runtime-model.md`
- `docs/architecture.md`
- `src/bootstrap/index.ts`
