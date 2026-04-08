Local guidance for `src/lib`.

## Scope

`src/lib` contains shared helpers such as:
- filesystem helpers
- prompt assembly
- config loading
- frontmatter parsing

## Rules

- Keep helpers deterministic and narrowly scoped.
- Avoid pulling ACP, Ink, or provider SDK concerns into this directory unless the helper is explicitly about that boundary.
- Shared filesystem helpers must preserve session-root safety and predictable traversal behavior.
- Prefer small utilities over broad utility grab-bags.

## Read First

- `docs/architecture.md`
- the specific helper file you are changing
