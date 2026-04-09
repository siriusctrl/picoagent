Local guidance for `src/tools`.

## Scope

`src/tools` defines LLM-facing capabilities.

## Rules

- Keep tools focused: one tool, one capability.
- Validate parameters with schema checks at the boundary.
- Prefer shared helpers for path resolution and filesystem behavior.
- Do not scatter agent gating into tool implementations when the registry boundary is enough.
- If a tool contract changes, add or update deterministic tests.

## Read First

- `docs/runtime-model.md`
- `src/core/tool-registry.ts`
- the specific tool file you are changing
