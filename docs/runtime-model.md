# Runtime Model

The runtime model is built around a simple split:

- `workspace` is the source of files and control inputs
- `session` owns context
- `run` is one execution through the runtime
- resources such as workspace files or session history stay readable without having to force everything into the live prompt

## Workspace

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
- bound workspace root
- optional additional roots
- conversation history
- default agent preset
- cached control snapshot
- optional active checkpoint plus recent tail messages

The control snapshot is derived from workspace control files such as:
- `.pico/config.jsonc`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `.pico/memory/`
- `skills/`
- `agents/`

Each run records its own agent preset.
Session runs inherit the session default unless the request overrides it.
Before a session run starts, the runtime checks whether the bound workspace changed and refreshes the control snapshot automatically if needed.

There is no worker graph behind the session.
There is one agent loop for the session.

HTTP is the durable runtime surface.
Any UI around it should stay replaceable.

The session is not owned by one request handler.
Runtime state lives behind a store boundary and is projected back out as session and run snapshots.

What is still missing:
- session state is not yet durable across process restarts
- session history is not yet exposed as an HTTP resource surface, only through tools
- control snapshots are still rebuilt from the local filesystem rather than a general workspace resource contract

## Session History

Session compaction does not replace the session.

Instead it creates a checkpoint summary that covers older messages while the session keeps a recent tail for live interaction.
New runs use:
- cached control snapshot
- latest checkpoint summary when present
- tail messages after that checkpoint

Full history still remains available for inspection as session resources:
- `summary.md`
- `checkpoints/<id>.md`
- `runs/<id>.md`
- `events/<runId>.ndjson`

## Agent Presets

`picoagent` has two built-in agent presets:

### `ask`

Equipped tools:
- `list_files`
- `read_file`
- `search_text`
- `list_session_resources`
- `read_session_resource`

Use it for:
- exploration
- search
- explanation
- planning

### `exec`

Equipped tools:
- everything in `ask`
- `compact_session`
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
- file reads and writes go through the environment and its workspace filesystem implementation
- command execution goes through local command execution
- filesystem traversal and text search use local deterministic helpers

Today this means:
- tool-facing workspace files can be virtualized behind the workspace filesystem boundary
- session control snapshots are still built from the local workspace directly
- `run_command` is still an OS process boundary rather than a virtual workspace command layer
