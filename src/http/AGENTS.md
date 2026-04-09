Local guidance for `src/http`.

## Scope

`src/http` owns the minimal async HTTP adapter over the shared Pico runtime.

## Rules

- Keep the endpoint surface narrow and explicit.
- Keep the resource model explicit: sessions, runs, and events.
- Prefer direct request handlers over framework-looking abstractions.
- Reuse the same bootstrap path and loop semantics as other transports.
- Add deterministic tests for any new endpoint or event contract.

## Read First

- `docs/entrypoints.md`
- `docs/architecture.md`
- `src/http/server.ts`
