# Entrypoints

## Local TUI

Entry file: `src/tui/main.tsx`

Behavior:
- starts the Ink UI
- spawns the ACP agent as a child process
- creates one ACP session rooted at the current working directory
- renders streamed assistant output and tool activity
- lets the user switch between `ask` and `exec`

This is the default way to run the project.

## ACP Agent

Entry file: `src/acp/main.ts`

Behavior:
- starts an ACP agent on stdio
- loads config and prompt framing from the control workspace
- assembles provider plus tool registry
- serves one or more ACP sessions

The ACP agent is transport-only. It does not own a second UI.

## Shared Bootstrap

Both entrypoints rely on the same bootstrap path:

1. load config from the control workspace
2. create the provider
3. assemble the global tool registry
4. create ACP session state for each session
5. build the system prompt for the active mode

The important boundary is:
- bootstrap defines the agent shape
- ACP defines the transport
- Ink defines the local UI
