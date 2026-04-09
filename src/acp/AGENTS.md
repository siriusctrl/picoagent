Local guidance for `src/acp`.

## Scope

`src/acp` owns:
- stdio ACP entrypoint
- ACP session lifecycle
- ACP-backed environment
- mapping core loop events into ACP updates

This directory is the primary product surface.

## Rules

- Keep transport details here, not in `src/core`.
- Do not move model logic or provider-specific behavior into this directory.
- Session updates should stay deterministic and resilient to malformed model output.
- If ACP session behavior or tool-call reporting changes, add or update deterministic tests.
- Prefer growing the server contract here before adding client-specific behavior elsewhere.

## Read First

- `docs/runtime-model.md`
- `docs/entrypoints.md`
- `src/acp/session-agent.ts`
