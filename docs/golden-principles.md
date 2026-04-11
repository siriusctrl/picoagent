# Golden Principles

1. Keep `session`, `runtime`, `filesystem`, and `execution backend` boundaries explicit.
2. Keep one general tool registry for the whole app.
3. Let control files shape runtime behavior instead of built-in agent presets.
4. Keep sessions as context storage, not runtime policy containers.
5. Keep provider SDKs and transport details out of `src/core`.
6. Keep HTTP thin over the shared runtime.
7. Validate external input at the boundary.
8. Keep prompt framing in the control workspace.
9. Prefer direct, readable modules over framework-looking layers.
