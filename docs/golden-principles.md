# Golden Principles

1. Keep one agent loop for root and child runs.
2. Keep provider wire formats outside the loop.
3. Keep one sorted tool registry for built-ins, MCP, skills, and background work.
4. Preserve large outputs as artifacts; bound what enters model context.
5. Persist complete messages, not partial stream deltas.
6. Keep run directories portable and inspectable.
7. Keep memory as ordinary Markdown outside the transcript and load it on demand.
8. Keep stable prompt material ordered before dynamic results.
9. Keep the launch runtime headless and scheduler-free.
10. State plainly that host execution is not sandboxed.
