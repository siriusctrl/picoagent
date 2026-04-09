# Runtime Model

The runtime model is built around a simple split:

- `session` owns context
- `resource` provides file-backed inputs and, when supported, command execution
- `run` is one execution through the runtime
- a session may also project a read-only file-view so the model can inspect history without forcing everything into the live prompt

## Resource

The current concrete resource is the workspace rooted at the directory where you launch `picoagent`.

It owns prompt framing and local configuration:
- `.pico/config.jsonc`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `.pico/memory/`
- `skills/`
- `agents/`
- `.pico/runtime/`

This workspace is the current source of intent and the current executable target for `cmd`.

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
- control snapshots are still rebuilt from the local filesystem rather than a general resource contract

## Session History

Session compaction does not replace the session.

Instead it creates a checkpoint summary that covers older messages while the session keeps a recent tail for live interaction.
New runs use:
- cached control snapshot
- latest checkpoint summary when present
- tail messages after that checkpoint

For model-side inspection, the session exposes a read-only file-view:
- `summary.md`
- `checkpoints/<id>.md`
- `runs/<id>.md`

Raw event logs remain available to clients over HTTP session resources and run event endpoints.

## Agent Presets

`picoagent` has two built-in agent presets:

### `ask`

Equipped tools:
- `glob`
- `grep`
- `read`

`grep` is a case-insensitive literal line search with optional surrounding context lines.
On the executable workspace target it prefers `rg` when available and falls back to the built-in file-view search when it is not.

Use it for:
- exploration
- search
- explanation
- planning

### `exec`

Equipped tools:
- everything in `ask`
- `patch`
- `cmd`

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
- tool-facing resources can be virtualized behind the workspace filesystem boundary
- sessions can reuse the same file-view logic for read-only history inspection
- session control snapshots are still built from the local workspace directly
- `cmd` is still an OS process boundary rather than a virtual workspace command layer
