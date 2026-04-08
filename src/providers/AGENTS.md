Local guidance for `src/providers`.

## Scope

`src/providers` contains SDK adapters only.

## Rules

- Keep SDK imports in this directory, never in `src/core`.
- Translate between provider payloads and core message/tool shapes.
- Validate provider responses at the boundary before converting them into core types.
- Keep provider-specific quirks contained here instead of leaking them across the app.

## Read First

- `src/core/provider.ts`
- the provider adapter you are changing
