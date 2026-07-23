# Architecture

Fiasco is a headless Rust orchestrator for multiple agents and background jobs.
Its architecture favors a small, inspectable execution core over a UI or
platform framework.

## Runtime Flow

```text
job/CLI
  -> AgentRunner
     -> ModelProvider
     -> ToolRegistry
        -> local Tool adapters grouped where related
        -> MCP Tool adapters
        -> RuntimeHandleManager
           -> promoted direct Tool future
           -> delegated child AgentRunner
     -> ArtifactStore
     -> RunDirStore
        -> compacted-state metadata / compacted-history reader
        -> run-local planning graph files
     -> EventSink
```

`AgentRunner` is the only model/tool loop. It calls a provider, executes the
returned tool calls, appends complete tool results, and repeats until the model
returns a final answer with no unresolved background work. There is no model
step limit; provider stream-idle timeouts, request deadlines, and explicit
cancellation remain real failure boundaries.

## Core Boundaries

### Model provider

`ModelProvider` translates the canonical message and tool shapes to one wire
protocol. OAuth, API keys, SSE parsing, provider errors, and provider-specific
cache hints stay behind this boundary. The runner owns the non-resetting model
request deadline; streaming providers apply the per-call idle interval while
opening the HTTP request through its response headers and before each next valid
SSE event. A protocol-signalled structurally incomplete normal response is
discarded and retried once as a fresh request with an ephemeral tail reminder;
ordinary transport and provider errors remain terminal for that run.

Initial adapters:

- OpenAI OAuth
- OpenAI-compatible Responses or Chat Completions
- Anthropic-compatible Messages
- deterministic echo for tests and smoke runs

Canonical user content can include image attachments. Adapters project those
to native Chat `image_url`, Responses `input_image`, or Anthropic base64 source
blocks; the agent loop does not assemble provider wire shapes.

### Tool registry

Every model-callable action implements `Tool`. Local adapters and MCP adapters
share the same registry. Memory uses the ordinary file tools. The registry
caches each adapter's spec at registration, stays sorted, and is frozen before
the first normal provider call so tool schema order and membership remain
deterministic across requests.

This registry is the capability router: it maps a model-returned tool name to
one implementation and one schema. It does not decide what to do or create a
second planning layer; the model selects a capability, and the runner performs
the deterministic lookup. Duplicate names fail during startup instead of
silently replacing an existing capability.

Every local model-facing adapter keeps its typed compile-time `tool.yaml` beside
its Rust module. Standalone tools live directly under `src/tools/<tool>/`;
cohesive handle, history, and graph families live under
`src/tools/<family>/<member>/`. The manifest always contains the complete
provider-visible name; paths never derive names. The common loader validates
both prose fields and joins them with a `Returns:` semantic boundary into the
standard provider description. Its Rust module owns arguments, semantic
validation, and execution.
The base `bash` adapter uses a non-login shell and inherits the fiasco
process environment, avoiding per-call profile output and PATH rewrites.
The base `read` adapter returns up to 400 text lines under a 65,536-byte cap.
When the byte cap lands inside a multi-line range, it backs up to the newest
complete line and returns an exact continuation offset. Supported images are
artifacted and carried separately as canonical model attachments.
The loader rejects unknown manifest fields, empty or padded prose, and
non-object input schemas. Domain engines remain separate: process-local
execution coordination is owned by `RuntimeHandleManager`, skills by
`SkillRegistry`, and trajectory retrieval by
`TrajectoryReader`. MCP lifecycle and its server-provided dynamic adapter
remain in `mcp.rs`.

`build_app_tools` assembles process-wide local capabilities. `RunToolAssembly`
is the single path that adds run-scoped history, handle controls, and `delegate`
for every Root and GeneralTask run. The `history` and `handle` family modules
explicitly register their complete member sets; assembly does not repeat each
leaf constructor. Ordinary tools are called directly; only an unfinished direct
call receives a runtime handle through foreground promotion.
The model-visible schema set and resume hash therefore commit the same fixed
capability contract without a dynamic spawn allowlist.

The fixed built-ins also include `graph_init` and `graph_list`. Their shared
run-local store allocates short `g<N>` YAML files without overwriting a graph
during concurrent initialization. `graph_init` accepts the complete initial
node map, validates references and acyclicity, and creates no file when the
topology is invalid. `graph_list` parses each file independently,
validates its DAG and terminal state, and derives ready nodes; one malformed
file is reported as invalid rather than failing the entire listing. Full graph
inspection and mutation stay with `read` and `write`. Execution stays with
`delegate` and the existing handle controls, so the graph family does not create
a second scheduler or persist runtime handles. Ready nodes are projected only for a
`wip` graph, and an accepted resolution is invalid until its direct dependencies
are resolved. Since one assistant tool-call batch is concurrent, dependent
`write`, `graph_list`, and `delegate` stages execute in separate turns.

Root and the persisted delegating/leaf GeneralTask profiles have one identical
built-in capability set. Both GeneralTask profiles appear to the model as the
common GeneralTask role. Each normal run registers `history_search` and
`history_read` before its first call regardless of whether automatic compaction
is configured, plus `delegate` and all handle controls. Remaining delegation
depth is persisted, shown in the runtime reminder, and checked by `delegate`
before child creation; zero returns a local error. Optional `web_search` and MCP
tools depend on startup configuration. Memory paths do not add a tool schema.
The selected schemas do not appear or disappear during one run.

The provider config declares one capability set for the selected model rather
than maintaining a model-name registry or probing endpoints. The stable system
prompt defines absent modalities as unavailable; the initial runtime reminder
records the concrete set as `current model supported modalities: [...]`.
`read` receives the corresponding image-enabled flag during app-tool assembly
and returns a normal tool error for an image under a text-only configuration.
Its static YAML schema and description do not change.

### Run storage

Each run is a portable directory beneath `<workspace>/.fiasco/runs/<run-id>/`.
It contains run metadata, append-only complete messages, structured events, the
final answer, and artifacts. It may also contain
`graphs/g<N>.yaml` files whose nodes represent durable work-item topology and
main-agent-accepted outcomes. This is what persistence means in the launch
runtime:
a cloud worker can retain or inspect a job without a database.

The durable message contract is `fiasco-message`. Every `messages.jsonl` line is
self-contained: `ref`, `created_at`, `role`, and the exact typed
provider-neutral `content` blocks used by the runner. Tool-error state,
structured artifact refs, images, reasoning, runtime-handle results, and
opaque provider continuation items therefore need no second representation or
reconstruction layout. Optional pending-input idempotency and compaction state
live under `_fiasco` on the same record. The one-based sequence is derived from
the required `m<N>` ref and line position rather than stored again.

Assistant function-call `arguments` remain the exact provider string in this
log, including malformed JSON. Parsing occurs only at the individual tool
execution boundary. An invalid call receives an ordered `is_error` result while
valid sibling calls from the same assistant message continue normally.

Image attachments remain typed base64 content blocks in the log. Provider
adapters project text and images to their native wire shapes only when building
a request.

`run.json` identifies the format as `fiasco-message` and records the model
modality declaration, persisted profile, and remaining delegation depth. Resume
requires the current model declaration to match and restores delegation
authority from that run snapshot rather than current depth configuration.

Only complete checkpoints are resumable. Stream deltas are emitted to live
sinks but omitted from the persisted `events.jsonl` and are never appended as
partial conversation messages.

The one process holding the run execution lease is the sole writer. Each
newline completes one physical message record, while `_fiasco.checkpoint` groups
one or more records into the logical commit boundary. Read-only viewers take no
message-log lock and publish a group only when every declared line is complete.
The writer trims an incomplete tail group before resuming appends. Malformed
completed records and out-of-sequence refs fail loading. The current pre-release
format intentionally does not load older run-record versions.

One incremental checkpoint decoder owns this definition for every reader. Its
synchronous core validates complete physical lines from any known sequence and
byte offset, retains at most one incomplete checkpoint, and returns exact raw
lines only after the whole group commits. The async file wrapper retains a torn
final line across EOF and streams forward without reading the whole file;
trajectory loading collects its committed records, while append recovery only
counts them and uses the same committed byte offset.

### Transcript inspection

`fiasco inspect` is a read-only adapter over the same durable boundary:

```text
inspect command
  -> TranscriptTimeline (Fiasco storage/checkpoints/run state)
     -> fmtview::view::RecordTimeline
        -> fmtview (tail loading/search/navigation/render/terminal lifecycle)
```

The command is selected before application config and provider composition.
Summary and exact NDJSON paths therefore need only the run directory, and the
interactive path constructs no provider, MCP client, hooks, tools, or runner.
Fiasco depends on the released `fmtview` facade and does not depend directly on
`fmtview-core` or own ratatui/crossterm behavior.

The timeline opens at the last committed checkpoint by scanning physical lines
backward from EOF. It loads older checkpoints backward and newer checkpoints
forward; one logical checkpoint may exceed a requested record/byte budget when
it is the first group in a batch, but is never split across published batches.
Prefix probes are bounded. The initial tail path does not index or validate
every earlier message, so a large history can show its first screen without a
forward scan.

Follow refresh retains the shared `CheckpointDecoder`, its pending checkpoint,
a torn-line buffer, and the scanned suffix cursor. An unchanged large pending
checkpoint therefore costs only bounded head/middle/tail probes per refresh.
Rewriting only the uncommitted suffix invalidates and rebuilds this tracker from
the unchanged committed boundary without changing the epoch. Reads from a
concurrently shrinking or rewritten suffix are retried from a clean working
tracker and published only after one coherent file observation. Truncating a
committed prefix, replacing the file identity, or changing bounded committed
prefix probes starts a new epoch so fmtview discards old record identities.
Queued, running, and idle runs report a live boundary; completed, failed,
cancelled, and closed runs report a terminal boundary.

Interactive snapshot and follow modes are TTY-only. Redirected stdout defaults
to exact committed NDJSON, preserving every raw record byte and LF without
reserialization; `--summary` retains the metadata/final-output view. Events,
child-run trees, and handle controls are outside this transcript source.

The message file is created and directory-synced with the run. The writer's
cached next sequence is invalidated before cancellable I/O and restored only
after a complete record has synced. Multiple independent writers for one run
are outside the storage contract; the execution lease prevents that state in
the runtime.

The persisted run state is intentionally coarse; a failed or process-abandoned
run may re-enter `running` through the explicit resume command:

```text
queued -> running -> completed
                  `-> failed
failed/running -> running  # explicit resume, if not already owned
```

The loop injects newly completed runtime-handle results, optionally compacts an
old completed-message prefix, requests model output, persists the complete
assistant message, executes zero or more direct tool calls, persists their
results, then either repeats or completes.

Handle state is process-local execution coordination. A promoted tool uses a
`j_<ulid>` handle until that process ends. A delegated agent uses its child run
id as the handle; only the child transcript, display name, parent id, and
open/closed lifetime are durable. Current activities, followups, pending output,
and tool handles are not reconstructed after a crash.

A status-less `<runtime_handle>` in a tool result acknowledges asynchronous
execution in the original provider `tool_call_id` slot. Later output is one
user/runtime content block correlated by handle, never a second tool result
with the same provider call id. One message batches all outputs ready at that
boundary. Each body follows the ordinary independent
inline/preview/artifact policy; the XML status wrapper is added and escaped
afterward.

Before every normal provider request, the runner snapshots current-process
active handles in stable handle order. The synthetic reminder tells the model
not to delegate work already represented there and is never persisted. It is
refreshed after compaction so newly completed work is delivered before the next
normal request.

The full run holds a filesystem execution lease. Resume rebuilds the recorded
profile, validates provider/model/workspace identity, loads the message log and
latest complete checkpoint, and continues from there. A normal tool-turn
checkpoint contains the assistant message, every ordered tool result, and any
attachment message. An incomplete tail checkpoint is discarded as a unit and
replaced by a user/runtime warning about possible workspace or external side
effects; the call is never automatically replayed. Root restart also clears
pending input and unconditionally tells the model that the prior process and
its asynchronous work stopped.

### Artifact storage

Large foreground tool outputs are never discarded and do not enter the live
context in full. The store writes the complete bytes, records immutable
metadata, and gives the model a bounded beginning/end preview and a relative
path it can inspect in pages. Terminal background output uses the same policy:
small UTF-8 output stays inline and larger or binary output keeps the bounded
artifact envelope. Each result is limited independently; earlier output and
compaction do not change later representation. See
[artifacts.md](artifacts.md).

For immediate image reads, the runner puts every tool result from the batch in
assistant call order and one user attachment message into the same checkpoint.
This keeps native tool-call/result adjacency valid while still allowing several
concurrently read images to share one model input message.

### Context compaction and trajectory retrieval

Local compaction changes the active-context projection without rewriting prior
messages. `messages.jsonl` retains every committed completed message with a
stable `m<N>` ref whose number is its sequence, including the successful
compaction user instruction and exact assistant compacted state. Their `_fiasco`
state marks the pair and stores the covered prefix and first exact message kept.
A normal active
request excludes compaction instructions and older compacted states; it contains
the initial runtime message, newest exact assistant state, a short synthetic
user runtime reminder that identifies the state as context rather than a final
answer, and the exact recent ordinary suffix. The reminder exists only at an
active compaction boundary and is not appended to the trajectory.

The trigger uses a provider-neutral request estimate from the first call and
adopts provider-reported input usage whenever available. Configuring
`compact_at_tokens` controls compacted-state creation only; the normal system
prompt and history-tool schemas are already present and remain unchanged. The
additional request uses the same provider, model, system, and frozen schemas,
then appends one compaction user instruction. A returned tool call or empty
state is never executed or committed; fiasco records the invalid attempt and
retries that compaction request once.
Fiasco does not implement provider/server-side compaction.

`history_search` and `history_read` expose a read-only `TrajectoryReader`
boundary. The local implementation searches only messages outside the active
context, plus full textual artifacts linked to their tool results. Search uses
Rust regular expressions, returns newest matches up to a configured cap, and
has no cursor. Each match returns a sequence-addressed ref, a `source` that
distinguishes inline message content from a linked complete artifact, and a
bounded snippet. Read accepts that ref and a bounded before/after window,
returning chronological Chat-compatible JSONL and expanding when necessary to
preserve tool-call/result pairs. A future remote or database-backed trajectory
can implement the same reader without granting the model filesystem write
access.

For linked local artifacts, the query reads the structured `ArtifactRef` from
the completed message content. It does not parse the
model-facing preview prose or guess from a call id. The reader verifies the
artifact with a bounded-memory stream and invokes `rg` with bounded output. It
stops after the requested newest matches plus one, avoiding whole-artifact heap
loads and unnecessary older scans.

The launch local message source still materializes one run's trajectory JSONL
per history query. Artifact contents remain streamed and bounded. If run sizes
outgrow this simple backend, an indexed local or remote `TrajectoryReader` can
replace it without changing the model-facing tools.

A normal profile compacts only when both history tools and at least one generic
artifact inspection tool (`read` or `bash`) remain available,
preserving exact recovery as part of the compaction contract.

### Skills and instructions

The system prompt contains only stable built-in instructions loaded from
the typed `prompts/agents.yaml` registry. YAML folded scalars remove source-only
line wrapping before Rust receives each value. `src/prompts.rs` parses the
embedded registry once and rejects unknown or empty fields. Rust owns prompt
precedence, section ordering, dynamic values, and runtime-reminder framing.
Stable task-control relationships, compacted-history recovery guidance, and
planning-graph workflow live in that system prefix rather than Rust string
literals. The first user message's ordinary text `content` carries a
`<runtime-reminder>` block with model/workspace state, the workspace
`AGENTS.md`, sorted skill metadata, memory paths, and optional delegated
instructions, followed by the original request. A skill body enters the
conversation only after the model calls `load_skill`. That result omits the
already-catalogued name and description, and includes the absolute skill
directory so relative references remain resolvable. The `SKILL.md` entry path
is implied by that directory and is not repeated.

### Memory

Memory is durable knowledge about the user and projects. An ordinary agent's
runtime reminder exposes two Markdown locations. The normal `read`, `write`,
and `bash` tools inspect and update them. A large independent consolidation can
use the same durable GeneralTask child mechanism as other delegated work; an
external cron or job scheduler can invoke the convenience consolidation
command. See [memory.md](memory.md).

### Agent orchestration

A subagent is a child invocation of the same `AgentRunner`. It has its own run
directory and transcript, a `parent_run_id`, and a depth. The launch runtime runs
children in-process, shares the parent workspace and base tools, and caps depth
at one. “Shared workspace” means parent and child operate on the same working
project files; it is not a special second workspace abstraction. Child
transcripts stay out of the parent context; only bounded, sequenced activity
results and artifact references return to the parent.

All direct calls returned in one assistant message start concurrently and share
one foreground window. The runner returns as soon as all settle. At the
configured deadline, the same in-flight futures for only unfinished calls move
under process-local runtime handles without stopping or restarting. The runner resumes and
tracks every unfinished future before awaiting promotion events, then commits
tool-result messages in original call order with their original provider call
ids; events may show actual completion order. A promoted handle is only a
running acknowledgement, so dependent work waits for the separate result
message correlated by that handle.

`delegate` starts a reusable GeneralTask agent immediately and returns its
child run id as the handle. `list_handles` discovers durable direct children
and overlays current-process tool jobs and activity state. `status` inspects
selected handles, while `wait` uses wait-any semantics: it returns when any
selected handle has a result or status change, or when its bounded interval
expires. `inspect`, `send_message`, and `close` operate only on agents. Inspect
projects a bounded page of the child's durable messages. Send queues a normal
user message with an explicit mode: `steer` makes it available after the
current complete tool batch, while `followup` waits for the current activity
boundary. An activity result leaves the thread idle; `stop` ends only current
work, and `close` permanently closes an idle thread. Asynchronous work has no
hard execution deadline.

Every delegated child is isolated. It starts from its own runtime reminder and
the delegated prompt, which must contain the complete objective and any
task-specific context. The parent conversation, compaction state, and artifact
references are not copied. A child uses the configured GeneralTask model and
resumes solely from its own run messages through the same `AgentRunner` path.

There is no durable parent-side handle index or activity record. On root
restart, tool jobs, followups, active work, pending parent input, and
undelivered output from the previous process are discarded. The root receives
an unconditional crash reminder and decides what to retry. `list_handles`
finds child runs whose `parent_run_id` matches the current run without launching
them. The first explicit `send_message` to an old open child clears stale child
pending input, adds a child crash reminder, and starts a fresh activity from
that child's complete transcript.

This recovery path assumes the runtime supervisor, cgroup, or container killed
the previous fiasco process and all locally managed descendants before
resume. A stale busy lease fails immediately. Remote work and external side
effects can survive and must be inspected after the restart reminder.

The durable child guarantee belongs to `delegate` child runs, and the parent
run is the only resume entrypoint. Memory consolidation
uses this same path rather than a special direct-tool child.

## Prompt And Cache Shape

Agent and compaction calls use one invariant built-in system prompt and one
sorted, frozen tool-schema set. The history schemas are included from the first
call; automatic compaction never mutates this prefix. Stable tool-family
workflow is part of the shared system prose, while project instructions, skill
metadata, memory paths, and delegated instructions form a deterministic runtime
reminder at the start of each run. That persisted reminder is frozen for the
run. Optional startup schemas are selected before the run starts. Agent role
and remaining delegation depth change only the initial runtime-reminder tail.
Current-process active-handle state is a non-durable synthetic reminder in
normal requests, while a compaction request changes only the message tail.

The durable trajectory remains append-only; before a normal model call, an
optional assistant compacted-state message can replace its older active prefix
while retaining the exact recent suffix. Large outputs become immutable
artifacts with bounded previews. These choices bound request growth while
keeping raw evidence inspectable and making provider KV-cache reuse possible
without making cache behavior part of the core API.

### Hooks

Command hooks observe `run_start`, `run_end`, `tool_before`, and `tool_after`.
They receive JSON over stdin and inherit the host process permissions. For a
root, `run_end` follows terminal completion; for a reusable child it follows
each completed activity after the child becomes idle, not explicit close.
Hooks do not define a second execution path.

## Runtime And Inspector Surfaces

The execution path emits NDJSON runtime events for machines and a compact final
result for humans. The separate read-only inspect path can embed fmtview's TUI,
but no terminal state enters `AgentRunner` or storage contracts. A future API or
web client should consume the same runtime events and run artifacts rather than
introduce model logic in the transport.

## Deliberate Launch Omissions

- OS sandbox and interactive approvals
- browser frontend or a second transcript rendering stack
- built-in scheduler
- vector search
- provider/server-side compaction
- database-backed run indexing
- native dynamic plugins
- distributed subagents

These omissions reduce launch complexity. Existing boundaries allow them to be
added without creating another agent loop.
