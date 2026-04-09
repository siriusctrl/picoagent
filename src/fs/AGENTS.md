Local guidance for `src/fs`.

## Scope

`src/fs` contains deterministic filesystem helpers.

## Rules

- Preserve session-root safety and predictable traversal behavior.
- Keep helpers deterministic and free of ACP or UI concerns.
- Prefer small filesystem primitives over broad catch-all utilities.

## Read First

- `docs/architecture.md`
- `src/fs/filesystem.ts`
