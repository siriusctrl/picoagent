Local guidance for `src/config`.

## Scope

`src/config` owns configuration loading and provider environment resolution.

## Rules

- Keep config parsing deterministic and narrowly scoped.
- Avoid pulling HTTP, Ink, or provider SDK concerns into this directory.
- Prefer explicit config behavior over broad utility abstractions.

## Read First

- `docs/architecture.md`
- `src/config/config.ts`
