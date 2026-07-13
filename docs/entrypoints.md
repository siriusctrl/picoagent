# Entrypoints

Picoagent has one binary, `pico`, and a reusable library.

## Commands

- `pico run`: execute a task and persist its run directory.
- `pico inspect`: print run metadata and final output.
- `pico auth login`: OpenAI OAuth device login.
- `pico memory consolidate`: run model-driven user/project memory maintenance.
- `pico skills list`: inspect discovered skill metadata.

`pico run --output ndjson` emits transport-neutral runtime events. A future HTTP
or queue worker should call the library and forward the same events. It should
not create another model/tool loop.
