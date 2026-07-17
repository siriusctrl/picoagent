# Runtime Model

## Run

A run is one task executed by `AgentRunner`. Its states are queued, running,
completed, or failed. `pico resume <run-id>` continues a non-completed root run from
its last complete message. The implementation does not resume inside a provider
stream or shell command; the last complete message is the durable boundary.
One per-run execution lease prevents two processes from advancing the same
trajectory concurrently. Resume also requires the same non-secret provider
fingerprint: endpoint and wire protocol, plus provider-specific continuation
settings such as reasoning effort or Anthropic version. Credentials are never
part of that fingerprint.

## Durable Messages

`run.json` declares `message_format` as `openai-chat-compatible`.
`messages.jsonl` contains one complete OpenAI Chat-compatible message per line:
user messages have `role` and `content`; assistant messages have `role`,
`content`, optional `tool_calls`, and optional compatible-endpoint
`reasoning_content`; tool messages have `role`, `tool_call_id`, and `content`.
The runtime reminder is ordinary text inside the initial user `content`, so the
message file contains no `runtime_reminder` JSON type.

`reasoning_content` records reasoning text explicitly returned by a compatible
endpoint. It is a common endpoint extension, not an official OpenAI Chat
Completions field, and is absent when the provider does not return it. Later
compatible Chat requests replay it separately from visible assistant `content`.

Each message has one corresponding line in `message_metadata.jsonl`. The
sidecar holds the stable ref, sequence, timestamp, exact-message SHA-256,
provider-neutral content layout, tool-error state, opaque provider items, and a
result's optional `ArtifactRef` plus exact preview-byte count. A second SHA-256
covers all reconstruction metadata. The two logs are created and
directory-synced with the run. The Chat line is synced first and the metadata
line is synced last as its commit marker. Reads, recovery, and appends take one
per-run file lock, including across independently opened stores.
Readers expose only paired records whose hashes and layouts validate. A lone
final Chat message is treated as an interrupted append; metadata ahead of the
message log or corruption in a committed pair is an error. This pre-release
contract does not include a decoder for the previous private message envelope.

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

On resume, a complete final assistant message is finalized without another
model call. If the last assistant message contains a tool call without a paired
durable result, picoagent appends an interrupted error result and does not run
the tool again because its side effects are unknown.

## Compaction And History

Automatic local compaction is off unless `compaction.trigger_tokens` is set.
That option controls checkpoint creation only: every normal agent profile has
both history tool schemas from its first call, and neither its system prompt nor
toolset changes when a checkpoint appears. Compaction can trigger only after
the provider reports input-token usage. Between calls, picoagent estimates the
content adapters replay, including compatible Chat `reasoning_content` and
opaque provider continuation items. The summary call uses the same provider and
model with a separate system prompt, no tools, and a separate output limit; it
does not consume a normal model-step slot. A
summary failure emits a lifecycle event and leaves the prior/full context in
use. A fixed profile lacking either history tool or both generic artifact
inspection tools (`read` and `bash`) would keep the full context instead of
compacting without exact retrieval.

Compaction does not mutate committed pairs in `messages.jsonl` and
`message_metadata.jsonl`. It appends a checkpoint to `compactions.jsonl`, then
assembles later model requests from the initial runtime message, latest summary,
and exact recent messages. The raw trajectory therefore remains the source for
read-only recovery:

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

Task JSON is coordination state, not a second transcript. Delivery is derived
from `BackgroundTaskResult` entries already committed to the parent message
log. After restart, running ordinary tools become terminal `interrupted` tasks
and are never replayed. A queued/running child agent resumes its separate child
run with the same `AgentRunner`; completed or failed children are reconciled
into the parent exactly once.

The CLI resumes the parent, not a child id. Parent recovery owns durable
GeneralTask child reconciliation, which avoids two processes racing to advance
the same child. Large memory updates use this same child path and need no
separate recovery case.

Parent, child, and compaction requests share one model-call semaphore. Its
default capacity is one so a child can run against single-concurrency compatible
endpoints without racing the parent; deployments can raise it explicitly. Every
request also has the same configured deadline. A normal request timeout fails
that run; a compaction timeout records `compaction_failed` and continues with
the uncompacted context.

## Streaming

Provider text deltas are transient `model_delta` runtime events. Explicit
reasoning deltas are separate transient `model_reasoning_delta` events, so live
sinks can choose whether to render them. `RunDirStore` does not write either
per-chunk event to `events.jsonl`; that file retains lifecycle, tool, artifact,
usage, and failure events for inspection and debugging. Only the complete
assistant message, including optional `reasoning_content`, enters
`messages.jsonl`, preventing partial or duplicated content after a crash.
Reasoning is not included in `final.md`.

## Prompt Stability

The normal agent's built-in system prompt is workspace-independent, loaded from
the embedded typed YAML registry, and invariant across its calls. Sorted tool
schemas form the other stable request prefix and are frozen before the first
call. Core history schemas are included regardless of `trigger_tokens`. Root
and a depth-eligible GeneralTask may include delegation schemas; each
GeneralTask is assigned a delegating or leaf variant before it starts. Optional
web and MCP schemas depend on startup configuration. Memory adds reminder paths,
not a schema. Compaction summaries use a separate tool-free profile.

The first user message begins with a `<runtime-reminder>` text block containing
the workspace snapshot: path, `AGENTS.md`, sorted skill metadata, memory paths,
and optional delegated instructions. The original user request follows after a
blank line in the same ordinary Chat `content` string. YAML folds source-only
wrapping in built-in agent prompts; dynamic reminder inputs remain exact.

Tool output, background results, and later complete messages append at the
durable conversation tail. Files or configuration changed during a run are
observed by the next run rather than rewriting the stored trajectory. When
enabled, a local compaction checkpoint can replace an older prefix only in the
next provider request. That synthetic replacement puts recovery guidance
immediately before the `<compacted-history>` block; no guidance is sent when no
checkpoint exists. Large results remain behind stable artifact references.
These choices reduce context growth and improve the opportunity for provider-side
prefix-cache reuse without coupling the loop to one cache API.
