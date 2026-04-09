# Entrypoints

## ACP Server

Primary entry file: `src/acp/main.ts`

Behavior:

- starts an ACP agent on stdio
- loads config and prompt framing from the control workspace
- assembles provider plus tool registry
- serves one or more ACP sessions

This is the main product entrypoint.

## Local TUI

Secondary entry file: `src/clients/tui/main.tsx`

Behavior:

- starts a local Ink UI
- spawns the ACP server as a child process
- creates one ACP session rooted at the current working directory
- renders streamed assistant output and tool activity
- lets the user switch between `ask` and `exec`

This client is for local development and debugging. It should stay thin.

## Shared Bootstrap

Both entrypoints rely on the same bootstrap path:

1. load config from the control workspace
2. create the provider
3. assemble the global tool registry
4. create ACP session state for each session
5. build the system prompt for the active mode

The boundary is:

- bootstrap defines the agent shape
- ACP defines the transport
- clients stay replaceable
