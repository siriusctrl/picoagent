# Runtime Model

## Run

A run is one task executed by `AgentRunner`. Root runs use queued, running,
completed, or failed. Reusable child runs additionally use idle between
activities and closed after explicit lifetime termination. `fiasco resume
<run-id>` continues a non-completed root run from its last complete checkpoint.
The implementation does not resume inside a provider stream or shell command.
One per-run execution lease prevents two processes from advancing the same
trajectory concurrently. Resume also requires the same non-secret provider
fingerprint: endpoint and wire protocol, plus provider-specific continuation
settings such as reasoning effort or Anthropic version. Credentials are never
part of that fingerprint. The run separately records its configured model
modalities and rejects resume when they change.

## Durable Messages

`run.json` declares `message_format` as `fiasco-message`. `messages.jsonl`
contains one complete provider-neutral message per line. Each record has its
run-local `m<N>` ref, timestamp, role, and typed content blocks. The blocks
directly represent runtime reminders, text, images, reasoning, tool calls and
results, provider continuation items, and background-task notices. Tool errors
and `ArtifactRef` values remain attached to their result blocks. Optional
pending-input idempotency, compaction state, and checkpoint membership use
`_fiasco` on the same line.

This self-contained representation is not a provider wire format. OpenAI Chat,
OpenAI Responses, and Anthropic adapters project it independently. Keeping the
runtime representation directly avoids a second metadata log, byte-span
layout, duplicated sequence, and reconstruction hashes.

The run execution lease permits one writer and any number of read-only viewers.
Each line records its checkpoint's first ref, index, and count. A singleton is a
one-message checkpoint. Readers publish a multi-message checkpoint only after
all of its newline-terminated lines are present and contiguous. Before the next
append, the writer validates the complete prefix and trims the whole incomplete
tail checkpoint. Malformed committed JSON and a ref that does not match its
one-based line fail loading. This pre-release contract does not decode older
run-record versions.

## Loop

For each model step:

1. append newly ready background activity results to the current messages;
2. if compaction is enabled and the tracked usage threshold is reached, send
   the native older prefix plus one compaction user instruction, then commit
   the instruction and assistant compacted state as one checkpoint;
3. assemble the active context and send sorted tool schemas to the provider;
4. stream visible text and explicitly returned reasoning as separate events
   while collecting the complete response;
5. if the assistant is final, persist it as a singleton checkpoint;
6. otherwise execute its requested tools concurrently under one shared
   foreground window;
7. artifact large outputs, then commit the assistant, complete tool messages in
   original call order, and optional user attachment message as one checkpoint;
9. repeat, join outstanding background work before finalization, or write
   `final.md` when no tool calls or tasks remain.

On resume, a complete final assistant checkpoint is finalized without another
model call. An incomplete tool-turn checkpoint is discarded in full. Fiasco
appends a user/runtime reminder that uncommitted work may have changed the
workspace or external systems, and the model must inspect state before retrying.
It does not synthesize missing tool results or automatically replay the turn.

## Compaction And History

Automatic local compaction is off unless `compaction.compact_at_tokens` is set.
That option controls compacted-state creation only: every agent profile has
both history tool schemas from its first call, and neither its system prompt nor
toolset changes when a compacted state appears. Fiasco estimates the system,
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

Compaction does not mutate committed messages. After a successful response, it
commits the compaction user message and exact assistant compacted-state message
together; each record's `_fiasco` state distinguishes control from ordinary
conversation. Normal
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

## Delegated Agents

`delegate` starts a reusable general-task agent asynchronously. Each agent is a
task backed by one normal child run with a parent id. Children share the
workspace, provider, and base tools. The persisted delegating/leaf profile and
exact remaining delegation depth protect resume semantics; both profiles expose
the same built-in schemas. The default maximum depth of one gives the initial
child zero remaining depth. `task_wait` is bounded wait-any: it returns when any
selected task becomes inactive or the interval expires, without cancelling
unfinished work.

`delegate` accepts a short name and a self-contained prompt. Every child is
isolated: it starts with its own runtime reminder and delegated task, without
copying the parent conversation, compaction state, or artifact references.
The prompt must therefore include the complete objective and task-specific
context. The child uses the configured GeneralTask model and records a normal,
independent trajectory.

Child recovery uses that same local trajectory and the same `AgentRunner` as a
root run. It does not consult parent messages; only parent-child coordination
and eventual result delivery remain in the parent task record.

`task_inspect` returns a child's latest durable Chat-compatible messages and
can page backward by sequence. `task_send` always requires `mode`. `steer`
queues a normal user message after the current complete assistant/tool batch;
`followup` waits for the current activity output before automatically resuming
the same child. Neither mode interrupts a tool batch, and an idle agent starts
immediately in either mode. Activity completion produces an ordered output and
leaves the agent `idle`. `task_stop` interrupts only a current activity;
the resulting idle agent is paused until its next explicit `task_send`.
`task_close` permanently closes an idle agent and discards queued followups.
`task_status` reports all task state, including `paused`, while `task_list`
lists all agents owned by the current run. Task ids are
run-local: controls use ids returned by this run's `delegate`, `task_status`, or
`task_list`.

Task JSON is coordination state, not a second transcript. A record belongs to
the recoverable parent state only when its originating call has a ToolResult in
a complete parent checkpoint. Pre-checkpoint records and child directories are
ignored as orphans. Delivery is derived from each task's highest `output_seq` in
`BackgroundTask` entries already committed to the parent log. After restart,
recognized running ordinary tools become terminal `interrupted` tasks and are
never replayed. A recognized queued/running child activity produces an
`interrupted` output; its agent thread and child transcript become idle and
paused without launching a model call. Pending followups and steering remain
durable but wait for an explicit `task_send`, which reuses the same child
through `AgentRunner`. Normal in-process completion or failure can still
autoactivate queued followups. A closed child stays closed.

A status-less `<background_task>` tool result means only that work is running.
At a later model boundary, ready activity outputs are grouped in one
`<runtime-reminder>` user message. Each result block includes its task id, name,
status, and output sequence. Its payload follows the ordinary per-result policy: small
UTF-8 text stays inline, while large or binary output uses the bounded
`[Tool output]` artifact envelope. The runtime block is added after payload
limiting and its text is XML-escaped, so status, artifact metadata, read
instructions, and closing tags cannot be clipped or forged by task output.

The CLI resumes the parent, not a child id. Parent recovery owns durable
GeneralTask interruption and never advances a child automatically, which avoids
two processes racing to execute the same activity. Large memory updates use
this same child path and need no separate recovery case.

Resume has a process-domain precondition: the supervisor, cgroup, or container
has terminated the old fiasco process and all locally managed descendants.
A busy run lease is therefore an invariant violation, not a condition to poll.
Remote jobs and side effects outside that process tree may remain and are why
the restart reminder requires inspection.

Parent, child, and compaction requests share one model-call semaphore. Its
default capacity is one so a child can run against single-concurrency compatible
endpoints without racing the parent; deployments can raise it explicitly. Once
a call acquires a permit, the corresponding `model_started` or
`compaction_started` event is emitted and its hard request deadline covers the
entire provider call without resetting. Every started request closes with a
completed or failed event before the permit is released, so event order also
reflects the configured concurrency. A normal failure emits `model_failed`
before the enclosing `run_failed`; when a discarded incomplete response reports
usage, that failure retains its input, output, cached-input, and reasoning
counts. Each real compaction retry is a separate numbered attempt: invalid responses retain their reported input, output,
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

If a provider explicitly reports a structurally incomplete normal response
(for example, an output-token stop or a stream ending without its terminal
event), fiasco discards that partial assistant content and makes one repair
request. The second request reuses the same system prompt, frozen tools, and
existing messages with one non-durable runtime reminder appended at the tail.
Each real request emits its own started/failed or started/completed lifecycle,
including provider-reported usage for the discarded attempt when available.
Transport, authentication, filtering/refusal, malformed SSE, deadline, and
other provider errors do not use this repair path.

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
