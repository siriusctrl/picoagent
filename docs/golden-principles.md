# Golden Principles

1. Keep the kernel small and explicit.
2. Control-workspace intent beats execution-workspace convenience.
3. Provider SDKs stay out of `src/core`.
4. Validate untrusted inputs at the boundary, not everywhere.
5. Worker writes stay confined to the task workspace.
6. Prefer explicit callbacks over hidden global IO.
7. Stay understandable without introducing framework-looking layers.
