Local guidance for `src/prompting`.

## Scope

`src/prompting` owns prompt assembly and frontmatter-backed prompt scanning.

## Rules

- Keep prompt helpers deterministic and file-driven.
- Avoid mixing provider, HTTP, or client concerns into prompt construction.
- Prefer explicit prompt composition over hidden magic.

## Read First

- `docs/runtime-model.md`
- `src/prompting/frontmatter.ts`
- `src/prompting/prompt.ts`
