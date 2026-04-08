# Golden Principles

1. Keep one agent loop per session.
2. Keep one general tool registry for the whole app.
3. Let `ask` and `exec` equip tool subsets instead of changing the architecture.
4. Keep provider SDKs and ACP transport details out of `src/core`.
5. Use ACP as the client/agent contract.
6. Validate external input at the boundary.
7. Keep prompt framing in the control workspace.
8. Prefer direct, readable modules over framework-looking layers.
