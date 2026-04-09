# Golden Principles

1. Keep one agent loop per session.
2. Keep one general tool registry for the whole app.
3. Let `ask` and `exec` equip tool subsets instead of changing the architecture.
4. Let sessions carry a default agent preset while each run records the actual preset it used.
5. Keep provider SDKs and transport details out of `src/core`.
6. Keep HTTP thin over the shared runtime.
7. Validate external input at the boundary.
8. Keep prompt framing in the control workspace.
9. Prefer direct, readable modules over framework-looking layers.
