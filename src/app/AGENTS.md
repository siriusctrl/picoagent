Local guidance for `src/app`.

## Scope

`src/app` is bootstrap only.

## Rules

- Assemble config, provider, and tool registry here.
- Keep business logic out of bootstrap code.
- Prefer changing registry assembly over adding mode-specific special cases elsewhere.

## Read First

- `docs/runtime-model.md`
- `src/app/bootstrap.ts`
