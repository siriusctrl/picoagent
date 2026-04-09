# Runtime Model

## Control Workspace

The directory where you launch `picoagent`.

It owns prompt framing and local configuration:
- `.pico/config.jsonc`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `.pico/memory/`
- `skills/`
- `agents/`

This directory is the source of intent.

## Session

Each conversation is one HTTP session.

A session carries:
- `cwd`
- optional additional roots
- conversation history
- default agent preset

Each run records its own agent preset.
Session runs inherit the session default unless the request overrides it.

There is no worker graph behind the session.
There is one agent loop for the session.

HTTP is the durable runtime surface.
Any UI around it should stay replaceable.

The session is not owned by one request handler.
Runtime state lives behind a store boundary and is projected back out as session and run snapshots.

## Agent Presets

`picoagent` has two built-in agent presets:

### `ask`

Equipped tools:
- `list_files`
- `read_file`
- `search_text`

Use it for:
- exploration
- search
- explanation
- planning

### `exec`

Equipped tools:
- everything in `ask`
- `write_file`
- `run_command`

Use it for:
- implementation
- edits
- verification commands

## Tool Execution

The tool registry is global.

Agent changes do not swap out the runtime.
They only change which tools are available for the run.

That keeps the model simple:
- one loop
- one registry
- one provider
- one transport

The local TUI does not get its own runtime model.
It is only one local HTTP client over the same session boundary.

## Events

Run events are append-only records for one execution.

They are the shared source for:
- run snapshots
- run event reads
- SSE streaming to clients

Interactive clients should still primarily follow one run at a time.
Session-wide event feeds can exist later without changing the underlying runtime model.

## Environment Boundary

For tool execution:
- file reads and writes go through the local environment
- command execution goes through local command execution
- filesystem traversal and text search use local deterministic helpers
