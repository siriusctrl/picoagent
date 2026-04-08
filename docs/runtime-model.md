# Runtime Model

## Control Workspace

The directory where you launch `picoagent`.

It owns prompt framing and local configuration:
- `config.md`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `memory/`
- `skills/`
- `agents/`

This directory is the source of intent.

## ACP Session

Each conversation is one ACP session.

A session carries:
- `cwd`
- optional additional roots
- conversation history
- current mode

There is no worker graph behind the session.
There is one agent loop for the session.

## Mode Model

`picoagent` has two modes:

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

Mode changes do not swap out the runtime.
They only change which tools are available for the session.

That keeps the model simple:
- one loop
- one registry
- one provider
- one transport

## Environment Boundary

For tool execution:
- file reads and writes go through ACP client capabilities
- command execution goes through ACP terminals
- filesystem traversal and text search use local deterministic helpers

This keeps the client in control of actual writes and terminal processes without reintroducing a second agent role.
