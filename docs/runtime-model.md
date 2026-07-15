# Runtime Model

## Run

A run is one task executed by `AgentRunner`. Its launch states are queued,
running, completed, or failed. The implementation does not resume inside a
provider stream or shell command; the last complete message is the durable
boundary.

## Loop

For each model step:

1. append newly completed background results to the current messages;
2. send sorted tool schemas to the active provider;
3. stream visible text and explicitly returned reasoning as separate events
   while collecting the complete response;
4. persist the complete assistant message;
5. execute requested tools sequentially;
6. artifact large outputs and persist complete tool messages;
7. repeat, join outstanding background work before finalization, or write
   `final.md` when no tool calls or tasks remain.

## Tool Results

Tool errors are returned to the model as error tool results instead of
immediately aborting the loop. Runtime/store/provider failures fail the run.

## Subagents

`spawn` runs independent tools or general-task child agents concurrently up to
the configured limit. Each child creates a normal run with a parent id. Children
share the workspace, provider, and base tools. The default maximum depth of one
keeps the initial execution model predictable. `wait` is a bounded join; a wait
timeout does not cancel the task.

## Streaming

Provider text deltas are transient `model_delta` runtime events. Explicit
reasoning deltas are separate transient `model_reasoning_delta` events, so live
sinks can choose whether to render them. `RunDirStore` does not write either
per-chunk event to `events.jsonl`; that file retains lifecycle, tool, artifact,
usage, and failure events for inspection and debugging. Only the complete
assistant message, including any separate reasoning block, enters
`messages.jsonl`, preventing partial or duplicated content after a crash.
Reasoning is not included in `final.md`.

## Prompt Stability

The built-in system prompt is workspace-independent and compiled from a
Markdown asset. Sorted tool schemas form the other stable request prefix and
are frozen for the run. The first user message begins with a
`runtime_reminder` content block containing the workspace snapshot: path,
`AGENTS.md`, sorted skill metadata, memory paths, and optional delegated
instructions. The original user request follows after a blank line.

Tool output, background results, and later complete messages append at the
conversation tail. Files or configuration changed during a run are observed by
the next run rather than rewriting an earlier prefix. Large results remain
behind stable artifact references. These choices reduce context growth and
improve the opportunity for provider-side prefix-cache reuse without coupling
the loop to one cache API.
