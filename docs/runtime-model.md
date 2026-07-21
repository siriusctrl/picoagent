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
part of that fingerprint. The run separately records its configured model
modalities and rejects resume when they change.

## Durable Messages

`run.json` declares `message_format` as `openai-chat-compatible`.
`messages.jsonl` contains one complete OpenAI Chat-compatible message per line:
user messages have `role` and string or native multimodal `content`; assistant messages have `role`,
`content`, optional `tool_calls`, and optional compatible-endpoint
`reasoning_content`; tool messages have `role`, `tool_call_id`, and `content`.
The runtime reminder is ordinary text inside the initial user `content`, so the
message file contains no `runtime_reminder` JSON type.

`reasoning_content` records reasoning text explicitly returned by a compatible
endpoint. It is a common endpoint extension, not an official OpenAI Chat
Completions field, and is absent when the provider does not return it. Later
compatible Chat requests replay it separately from visible assistant `content`.

Each message has one corresponding line in `message_metadata.jsonl`. The
sidecar holds the stable `m<N>` ref whose number equals the one-based sequence,
timestamp, exact-message SHA-256, provider-neutral content layout, tool-error
state, opaque provider items, an optional pending-input idempotency id, optional
compaction purpose/boundary state, and a result's optional
`ArtifactRef`. A second SHA-256
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
2. if compaction is enabled and the tracked usage threshold is reached, send
   the native older prefix plus one compaction user instruction, then persist
   the instruction and assistant compacted state as completed messages;
3. assemble the active context and send sorted tool schemas to the provider;
4. stream visible text and explicitly returned reasoning as separate events
   while collecting the complete response;
5. persist the complete assistant message;
6. execute the assistant's requested tools concurrently under one shared
   foreground window;
7. artifact large outputs and persist complete tool messages in original call
   order;
8. if a direct result carries images, persist one user attachment message after
   the complete tool-result batch;
9. repeat, join outstanding background work before finalization, or write
   `final.md` when no tool calls or tasks remain.

On resume, a complete final assistant message is finalized without another
model call. If a matching promoted task record exists for an unpaired tool
call, picoagent reconstructs its task acknowledgement and separately delivers
the durable terminal task result. Otherwise it appends an interrupted error
result and does not run the tool again because its side effects are unknown.

## Compaction And History

Automatic local compaction is off unless `compaction.compact_at_tokens` is set.
That option controls compacted-state creation only: every agent profile has
both history tool schemas from its first call, and neither its system prompt nor
toolset changes when a compacted state appears. Picoagent estimates the system,
frozen schemas, and active messages before the first request, then adopts
provider-reported input usage whenever available. Between calls it estimates
new content adapters replay, including compatible Chat `reasoning_content` and
opaque provider continuation items. The compaction call uses the same provider,
model, system prompt, and frozen tool schemas, with a separate output limit and
one final user instruction. A tool-call or empty state emits a lifecycle event
and is retried once without execution; a request error or two invalid responses
leave the prior/full context in use. A fixed profile lacking either history
tool or both generic artifact
inspection tools (`read` and `bash`) would keep the full context instead of
compacting without exact retrieval.

Compaction does not mutate committed message pairs. After a successful response,
it appends the compaction user message and exact assistant compacted-state
message to `messages.jsonl`; their sidecar records distinguish control from
ordinary conversation and make the assistant record the commit marker. Normal
context assembly excludes the compaction instruction and older compaction
records, using the initial runtime message, latest exact assistant state, one
synthetic user `<runtime-reminder>` that says to continue rather than compact
again, and exact recent ordinary messages. The reminder is provider context,
not another durable message. The omitted ordinary trajectory remains the source
for read-only recovery:

- `history_search` uses one Rust regex and returns newest matches only from the
  compacted prefix, including matches in linked full textual artifacts. Each
  match has an `m<N>` sequence ref, a `message` or `artifact` source, and a
  bounded snippet;
- `history_read` returns chronological Chat-compatible JSONL around that ref
  and preserves tool-call/result pairs.

There is no cursor. The configured search maximum omits older query matches;
refine the regex to reach them. If the already-bounded tool response is itself
too large for model context, the normal artifact envelope preserves that full
response. These are different truncation boundaries. Provider/server-side
compaction is not implemented. If configured, `context_window_tokens` rejects a
normal or compaction request whose estimated input plus configured output
allowance reaches the nominal full window. The provider-neutral estimate is not
a tokenizer-exact guarantee.

## Tool Results

Tool errors are returned to the model as error tool results instead of
immediately aborting the loop. Runtime/store/provider failures fail the run.
Image reads are complete artifact-backed results plus native model attachments.
The runtime places attachments after every paired result from the assistant's
tool-call batch. Text reads return at most 400 lines under a 65,536-byte cap;
if a multi-line selection hits that cap, the partial trailing line is omitted
and the truncation marker gives the exact next offset.
The initial runtime reminder states the current model's supported modalities.
When `image` is absent, an image `read` returns a model-visible tool error before
loading the file, creating an artifact, or attaching content.

## Subagents

`delegate` starts a general-task child asynchronously. Each child creates a
normal run with a parent id. Children share the workspace, provider, and base
tools. The persisted delegating/leaf profile and exact remaining delegation
depth protect resume semantics; both profiles expose the same built-in schemas.
The default maximum depth of one gives the initial child zero remaining depth.
`task_wait` is a bounded join; a wait timeout does not cancel the task.
`task_stop` is the explicit cancellation operation.

`delegate` requires `context: "fresh" | "fork"`. Fresh preserves the isolated
child behavior. Fork freezes the durable parent message sequence immediately
before the assistant message that contains the delegate call, copies that full
trajectory prefix into the child, and appends the child runtime reminder and
task. Calls from the same assistant batch therefore share one fork boundary.
Compaction request/state metadata is copied so the child's first active model
projection is exactly the parent's request messages plus its task suffix; the
child does not immediately compact that inherited first request again.
Pending-input ids are intentionally cleared because they are run-local
steering idempotency keys, not model context.

Inherited user messages retain applicable facts and constraints, but are
background rather than active child instructions. The common stable system
prompt gives the appended delegated task precedence over conflicting ancestor
workflow, while the dynamic GeneralTask reminder only identifies the child and
its paired task. A child therefore does not repeat ancestor orchestration,
delegation, task control, or edits unless its own delegated task explicitly
requires them; later direct steering may refine that scope.

The local delegated assignment is the ordinary message at the current run's
fork boundary plus one. If compaction moves it before the recent kept tail,
normal active context pins the exact message after compacted state and its
continuation reminder. Every later compaction input also includes it exactly
once, preserving scope across repeated compaction and resume. Nested forks pin
only their own innermost assignment; projection does not append another
durable message.

Once the frozen prefix is complete, child recovery validates and uses only its
local run files; an interrupted partial copy can be completed from the recorded
parent boundary. Fork inherits the parent's selected model. Provider-reported
cached input usage remains observable in `model_completed` and
`compaction_completed` events, but the harness neither predicts nor fabricates
a cache hit.

`task_inspect` returns a child's latest durable Chat-compatible messages and
can page backward by sequence. `task_steer` queues a normal user message after
the current assistant/tool batch and before the next provider call. It does not
interrupt the current tool batch. `task_status` reports state without adding
an explanatory pseudo-message. Task ids are run-local: controls use ids
returned by this run's `delegate` or `task_status`, never an inherited
ancestor's coincidentally named `t<N>` id.

Task JSON is coordination state, not a second transcript. Delivery is derived
from `BackgroundTask` entries already committed to the parent message
log. After restart, running ordinary tools become terminal `interrupted` tasks
and are never replayed. A queued/running child agent resumes its separate child
run with the same `AgentRunner`; completed or failed children are reconciled
into the parent exactly once.

A status-less `<background_task>` tool result means only that work is running.
At a later model boundary, terminal records are grouped in one
`<runtime-reminder>` user message. Each terminal block includes its task id,
name, and status, while the body is only the workspace-relative path to the
complete result artifact. Completed output and failure, cancellation, or
interruption details all use this same artifact-only delivery path.

The CLI resumes the parent, not a child id. Parent recovery owns durable
GeneralTask child reconciliation, which avoids two processes racing to advance
the same child. Large memory updates use this same child path and need no
separate recovery case.

Parent, child, and compaction requests share one model-call semaphore. Its
default capacity is one so a child can run against single-concurrency compatible
endpoints without racing the parent; deployments can raise it explicitly. Once
a call acquires a permit, the corresponding `model_started` or
`compaction_started` event is emitted and its hard request deadline covers the
entire provider call without resetting. Every started request closes with a
completed or failed event before the permit is released, so event order also
reflects the configured concurrency. A normal failure emits `model_failed`
before the enclosing `run_failed`. Each real compaction retry is a separate
numbered attempt: invalid responses retain their reported input, output,
cached-input, and reasoning usage in `compaction_failed`, and a successful
`compaction_completed` carries the accepted attempt number. A compaction
rejected by the context-window preflight has no started event, a null attempt,
and no usage because no provider request occurred. Waiting for a permit does
not emit a started event. A separate stream-idle interval covers response
headers and the request opening that precedes them, then restarts after every
valid SSE event, so a healthy long reasoning response can outlive one idle
interval. Neither timer includes later tool execution. An expired normal call
fails that run; an expired compaction call records `compaction_failed` and
continues with the uncompacted context.

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
call. Core history schemas are included regardless of `compact_at_tokens`. Root
and GeneralTask receive identical built-in schemas, including delegation and
task controls. Remaining delegation depth is runtime state; it appears in the
initial reminder and a zero-depth `delegate` call fails before task creation.
Optional web and MCP schemas depend on startup configuration. Memory adds
reminder paths, not a schema. A compaction call reuses the same stable prefix.

The first user message begins with a `<runtime-reminder>` text block containing
the workspace snapshot: path, `AGENTS.md`, sorted skill metadata, memory paths,
and optional delegated instructions. The original user request follows after a
blank line in the same ordinary Chat `content` string. YAML folds source-only
wrapping in built-in agent prompts; dynamic reminder inputs remain exact.

Tool output, background results, and later complete messages append at the
durable conversation tail. Files or configuration changed during a run are
observed by the next run rather than rewriting the stored trajectory. When
enabled, the latest stored assistant compacted state replaces an older prefix
only in the projected provider context; the stored compaction instruction is
not replayed in normal requests. A synthetic runtime reminder immediately after
the state only identifies it as continuation context rather than a final answer
or another compaction request. The stable system prompt owns history-tool
guidance. Large results remain behind stable artifact references.
These choices reduce context growth and improve the opportunity for provider-side
prefix-cache reuse without coupling the loop to one cache API.
