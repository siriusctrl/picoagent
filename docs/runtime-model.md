# Runtime Model

The runtime model is built around a simple split:

- `session` owns context
- `runtime` assembles prompts, decides when to compact, and executes runs
- `filesystem` provides file-backed inputs
- `execution backend` provides command execution
- `run` is one execution through the runtime
- a session may also project a read-only file-view so the model can inspect history without forcing everything into the live prompt

Use `docs/design-choices.md` as the single source of truth for why these boundaries exist and which alternatives were intentionally rejected.

## Filesystem

The current concrete filesystem is the workspace rooted at the directory where you launch `picoagent`.

It is the main writable file-backed surface for both prompt inputs and tool access:
- `.pico/config.jsonc`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `.pico/memory/`
- `skills/`
- `agents/`
- `.pico/runtime/`

This workspace is the current source of intent and the current executable target for `cmd`.
Host-level defaults such as `$HOME/.pico/*` and bundled defaults are also read during control loading, but they are not modeled as a separate mounted surface in the current runtime.

## Session

Each conversation is one HTTP session.

In the local default harness, the runtime can still create sessions directly.
For explicit isolation, a session may instead live behind a dedicated session service that is started before the runtime and then bound into the runtime over HTTP.

A session carries:
- bound workspace root in the current local implementation
- optional additional roots
- conversation history
- default agent preset
- cached control state from the latest successful load
- optional active checkpoint plus recent tail messages

The control state is derived from workspace and host control files such as:
- `.pico/config.jsonc`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `.pico/memory/`
- `$HOME/.pico/config.jsonc`
- `$HOME/.pico/memory/`
- `skills/`
- `agents/`

Each run records its own agent preset.
Session runs inherit the session default unless the request overrides it.
Before a session run starts, the runtime checks whether the bound workspace changed and refreshes the cached control state automatically if needed.

The session is not responsible for prompt assembly.
The runtime reads session state, decides whether compaction is needed, reloads control inputs when needed, and builds the final prompt for one run.

There is no worker graph behind the session.
There is one agent loop for the session.

HTTP is the durable runtime surface.
Any UI around it should stay replaceable.

The session is not owned by one request handler.
Runtime state lives behind a store boundary and is projected back out as session and run snapshots.

When the runtime is bound to an external session service:

- clients still create sessions through the runtime API
- the runtime forwards those creates into the external session service instead of storing them locally
- the runtime consumes session state over the session-store boundary
- run state still lives locally in the runtime, but session-facing run snapshots and event logs are mirrored back into the session service so `/session/...` remains consistent

What is still missing:
- session history is still projected through a dedicated read-only file-view rather than a general mounted filesystem namespace

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

This projection is for browsing context, not for representing the session itself as a writable filesystem.
Raw event logs remain available to clients over HTTP session resources and run event endpoints.
The tool surface addresses these mounted views through namespace paths such as `/workspace/...` and `/session/...`.

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

## Runtime Boundaries

For tool execution:
- file reads and writes go through the filesystem boundary
- command execution goes through the execution backend
- filesystem traversal and text search use local deterministic helpers

Today this means:
- tool-facing files can be virtualized behind the workspace filesystem boundary
- sessions can reuse the same file-view logic for read-only history inspection
- control loading reads workspace files through the injected filesystem boundary and host defaults through the local host filesystem
- `cmd` is still an OS process boundary rather than a virtual workspace command layer
