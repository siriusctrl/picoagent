# Runtime Model

## Run

A run is one task executed by `AgentRunner`. Its launch states are queued,
running, completed, or failed. The implementation does not resume inside a
provider stream or shell command; the last complete message is the durable
boundary.

## Loop

For each model step:

1. append newly completed background results to the current messages;
2. if checkpointing is enabled and the provider-reported usage threshold is
   reached, use a separate tool-free request to summarize an older
   completed-message prefix into a compaction checkpoint;
3. assemble the active context and send sorted tool schemas to the provider;
4. stream visible text and explicitly returned reasoning as separate events
   while collecting the complete response;
5. persist the complete assistant message;
6. execute requested tools sequentially;
7. artifact large outputs and persist complete tool messages;
8. repeat, join outstanding background work before finalization, or write
   `final.md` when no tool calls or tasks remain.

## Compaction And History

Automatic local compaction is off unless `compaction.trigger_tokens` is set.
That option controls checkpoint creation only: every normal agent profile has
both history tool schemas from its first call, and neither its system prompt nor
toolset changes when a checkpoint appears. Compaction can trigger only after
the provider reports input-token usage. Between calls, picoagent estimates only
content the adapters replay, excluding diagnostic reasoning text. The summary
call uses the same provider and model with a separate system prompt, no tools,
and a separate output limit; it does not consume a normal model-step slot. A
summary failure emits a lifecycle event and leaves the prior/full context in
use. A fixed profile lacking either history tool or both generic artifact
inspection tools (`read` and `bash`) would keep the full context instead of
compacting without exact retrieval.

Compaction does not mutate `messages.jsonl`. It appends a checkpoint to
`compactions.jsonl`, then assembles later model requests from the initial
runtime message, latest summary, and exact recent messages. The raw trajectory
therefore remains the source for read-only recovery:

- `history_search` uses one Rust regex and returns newest matches only from the
  compacted prefix, including matches in linked full textual artifacts;
- `history_read` returns a bounded window around a stable ref and preserves
  tool-call/result pairs.

There is no cursor. The configured search maximum omits older query matches;
refine the regex to reach them. If the already-bounded tool response is itself
too large for model context, the normal artifact envelope preserves that full
response. These are different truncation boundaries. Provider/server-side
compaction is not implemented.

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

The normal agent's built-in system prompt is workspace-independent, compiled
from a Markdown asset, and invariant across its calls. Sorted tool schemas form
the other stable request prefix and are frozen before the first call. Core
history schemas are included regardless of `trigger_tokens`. Root and a
depth-eligible GeneralTask may include memory and delegation schemas; each
GeneralTask is assigned a delegating or leaf variant before it starts. Optional
web and MCP schemas depend on startup configuration. MemoryMaintenance uses a
narrow fixed profile. Compaction summaries use a separate tool-free profile.

The first user message begins with a `runtime_reminder` content block containing
the workspace snapshot: path, compacted-history recovery guidance, `AGENTS.md`,
sorted skill metadata, memory paths, and optional delegated instructions. The
original user request follows after a blank line.

Tool output, background results, and later complete messages append at the
durable conversation tail. Files or configuration changed during a run are
observed by the next run rather than rewriting the stored trajectory. When
enabled, a local compaction checkpoint can replace an older prefix only in the
next provider request; large results remain behind stable artifact references.
These choices reduce context growth and improve the opportunity for
provider-side prefix-cache reuse without coupling the loop to one cache API.
