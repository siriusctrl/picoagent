# Entrypoints

Fiasco exposes its multi-agent runtime through one binary, `fiasco`, and a
reusable library.

## Commands

- `fiasco run`: execute a task and persist its run directory.
- `fiasco inspect`: print run metadata and final output.
- `fiasco auth login`: OpenAI OAuth device login.
- `fiasco memory consolidate`: run model-driven user/project memory maintenance.
- `fiasco skills list`: inspect discovered skill metadata.

`fiasco run --output ndjson` emits transport-neutral runtime events. A future HTTP
or queue worker should call the library and forward the same events. It should
not create another model/tool loop.
