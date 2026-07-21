# Architecture

Picoagent is a headless Rust agent harness for local and cloud jobs. The launch
architecture favors a small, inspectable execution core over a UI or platform
framework.

## Runtime Flow

```text
job/CLI
  -> AgentRunner
     -> ModelProvider
     -> ToolRegistry
        -> local Tool adapters grouped where related
        -> MCP Tool adapters
        -> TaskManager
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
SSE event.

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
cohesive task, history, and graph families live under
`src/tools/<family>/<member>/`. The manifest always contains the complete
provider-visible name; paths never derive names. The common loader validates
both prose fields and joins them with a `Returns:` semantic boundary into the
standard provider description. Its Rust module owns arguments, semantic
validation, and execution.
The base `bash` adapter uses a non-login shell and inherits the picoagent
process environment, avoiding per-call profile output and PATH rewrites.
The base `read` adapter returns up to 400 text lines under a 65,536-byte cap.
When the byte cap lands inside a multi-line range, it backs up to the newest
complete line and returns an exact continuation offset. Supported images are
artifacted and carried separately as canonical model attachments.
The loader rejects unknown manifest fields, empty or padded prose, and
non-object input schemas. Domain engines remain separate: task state is owned
by `TaskManager`, skills by `SkillRegistry`, and trajectory retrieval by
`TrajectoryReader`. MCP lifecycle and its server-provided dynamic adapter
remain in `mcp.rs`.

`build_app_tools` assembles process-wide local capabilities. `RunToolAssembly`
is the single path that adds run-scoped history, task controls, and `delegate`
for every Root and GeneralTask run. The `history` and `task` family modules
explicitly register their complete member sets; assembly does not repeat each
leaf constructor. Ordinary tools are called directly; only an unfinished direct
call can enter task control through foreground promotion.
The model-visible schema set and resume hash therefore commit the same fixed
capability contract without a dynamic spawn allowlist.

The fixed built-ins also include `graph_init` and `graph_list`. Their shared
run-local store allocates short `g<N>` YAML files without overwriting a graph
during concurrent initialization. `graph_list` parses each file independently,
validates its DAG and terminal state, and derives ready nodes; one malformed
file is reported as invalid rather than failing the entire listing. Full graph
inspection and mutation stay with `read` and `write`. Execution stays with
`delegate` and the existing task controls, so the graph family does not create
a second scheduler or persist task ids. Ready nodes are projected only for a
`wip` graph, and an accepted resolution is invalid until its direct dependencies
are resolved. Since one assistant tool-call batch is concurrent, dependent
`write`, `graph_list`, and `delegate` stages execute in separate turns.

Root and the persisted delegating/leaf GeneralTask profiles have one identical
built-in capability set. Both GeneralTask profiles appear to the model as the
common GeneralTask role. Each normal run registers `history_search` and
`history_read` before its first call regardless of whether automatic compaction
is configured, plus `delegate` and all task controls. Remaining delegation
depth is persisted, shown in the runtime reminder, and checked by `delegate`
before task creation; zero returns a local error. Optional `web_search` and MCP
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

Each run is a portable directory beneath `<workspace>/.pico/runs/<run-id>/`.
It contains run metadata, append-only complete messages, structured events, the
final answer, artifacts, and background task records. It may also contain
`graphs/g<N>.yaml` files whose nodes represent durable work-item topology and
main-agent-accepted outcomes. This is what persistence means in the launch
runtime:
a cloud worker can retain or inspect a job without a database.

The durable message contract is `openai-chat-compatible`. Every
`messages.jsonl` line is one Chat-shaped user, assistant, or tool message;
runtime reminder text is part of the first user message's `content`, not a
custom JSON variant. Assistant reasoning explicitly returned by a compatible
endpoint uses the optional `reasoning_content` extension. This extension is not
part of the official OpenAI Chat Completions schema.

Text-only user messages keep string `content`. User image messages use the
native Chat content-part array with text followed by `image_url` data URLs;
their internal image layout is committed in `message_metadata.jsonl` so resume
reconstructs the canonical attachments exactly.

Picoagent-only state lives in the paired `message_metadata.jsonl` sidecar. Each
line carries a run-local `m<N>` message id whose number equals its one-based
sequence, a timestamp, SHA-256 of the exact Chat JSON, the layout needed to
recover provider-neutral content, and a second SHA-256 over that reconstruction
metadata. A queued steering input's idempotency id is stored separately when
present. Tool-error state and opaque provider continuation items also remain
there, as do structured result artifact refs. Compaction request/state
classification and state boundaries also live only in the sidecar. None of these
private fields are added to the Chat message. `run.json` identifies the format
as `openai-chat-compatible` and records the model modality declaration,
persisted profile, and remaining delegation depth. Resume requires the current
model declaration to match and restores delegation authority from that run
snapshot rather than current depth configuration.

Only complete messages are resumable. Stream deltas are emitted to live sinks
but omitted from the persisted `events.jsonl` and are never appended as partial
conversation messages.

Writing a message syncs its Chat line first, then syncs its metadata line. The
metadata line is the commit marker: loading exposes only paired, hash-valid
records. A lone final Chat line is an interrupted append and is removed before
the next append; metadata ahead of the message log, mismatched hashes, and
malformed completed records fail loading. The current pre-release format
intentionally does not load older run-record versions.

Both message files are created and directory-synced when the run is created.
Reads, recovery, and paired appends hold a per-run file lock, so two
`RunDirStore` instances cannot observe or produce half-interleaved commits. A
cached next sequence is trusted only when both durable file lengths still match;
it is removed before cancellable I/O and restored only after the metadata commit
has synced.

The persisted run state is intentionally coarse; a failed or process-abandoned
run may re-enter `running` through the explicit resume command:

```text
queued -> running -> completed
                  `-> failed
failed/running -> running  # explicit resume, if not already owned
```

The loop itself is a small state machine too: inject newly completed background
results, optionally compact an old completed-message prefix, request model
output, persist the complete assistant message, execute zero or more direct
tool calls, persist their results, then either repeat or complete. This makes
crash boundaries and event ordering explicit without introducing a workflow
engine.

Background tasks have a separate persisted state:

```text
queued -> running -> completed
                  |-> failed
                  |-> cancelled
                  `-> interrupted
```

Whether a terminal result has entered the parent context is derived from the
parent's committed `BackgroundTask` messages; task JSON does not carry a
second authoritative `delivered` flag. A `delegate` result is one normal tool
result. For an automatically promoted direct call, the running acknowledgement
fills the original provider `tool_call_id` slot with a status-less runtime
notice. Later completion is a new user/runtime message correlated by `task_id`,
never a second tool result with the same provider call id. One message batches
all terminal records ready at that boundary. Each terminal body is only its
complete artifact path; internal task state, not model-facing XML, retains the
originating call id and task kind.

The full run holds a filesystem execution lease. Resume rebuilds the recorded
profile, validates provider/model/workspace identity, loads the paired message
log and latest completed compacted state, and continues after the last completed
model step. An unpaired direct tool request becomes an explicit interrupted
error result and is never automatically replayed. If the call had already been
promoted, its durable task record instead reconstructs the missing task
acknowledgement and supplies the terminal result separately.

### Artifact storage

Large foreground tool outputs are never discarded and do not enter the live
context in full. The store writes the complete bytes, records immutable
metadata, and gives the model a bounded beginning/end preview and a relative
path it can inspect in pages. Terminal background output is artifact-only even
when small, so batched notices remain bounded. Each result is limited
independently; earlier output and compaction do not change later representation.
See [artifacts.md](artifacts.md).

For immediate image reads, the runner commits every tool result from the batch
first, in assistant call order, then commits one user attachment message. This
keeps native tool-call/result adjacency valid while still allowing several
concurrently read images to share one model input message.

### Context compaction and trajectory retrieval

Local compaction changes the active-context projection without rewriting prior
messages. `messages.jsonl` retains every committed completed message with a
stable `m<N>` ref whose number is its sequence, including the successful
compaction user instruction and exact assistant compacted state. Sidecar
metadata marks those two records and stores the covered prefix and first exact
message kept. A normal active
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
state is never executed or committed; picoagent records the invalid attempt and
retries that compaction request once.
Picoagent does not implement provider/server-side compaction.

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
the completed result's paired message metadata. It does not parse the
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
precedence, section ordering, dynamic values, and runtime-reminder framing. The
first user message's ordinary text `content` carries a `<runtime-reminder>`
block with the workspace `AGENTS.md`, sorted skill metadata, memory paths, and
optional delegated instructions, followed by the original request. A skill
body enters the conversation only after the model calls `load_skill`. That
result omits the already-catalogued name and description, and includes the
absolute skill directory so relative references remain resolvable. The
`SKILL.md` entry path is implied by that directory and is not repeated.

### Memory

Memory is durable knowledge about the user and projects. An ordinary agent's
runtime reminder exposes two Markdown locations. The normal `read`, `write`,
and `bash` tools inspect and update them. A large independent consolidation can
use the same durable GeneralTask child mechanism as other delegated work; an
external cron or job scheduler can invoke the convenience consolidation
command. See [memory.md](memory.md).

### Subagents

A subagent is a child invocation of the same `AgentRunner`. It has its own run
directory and transcript, a `parent_run_id`, and a depth. The launch runtime runs
children in-process, shares the parent workspace and base tools, and caps depth
at one. “Shared workspace” means parent and child operate on the same working
project files; it is not a special second workspace abstraction. Child
transcripts stay out of the parent context; only the bounded final result and
artifact reference return to the parent.

All direct calls returned in one assistant message start concurrently and share
one foreground window. The runner returns as soon as all settle. At the
configured deadline, the same in-flight futures for only unfinished calls move
into this task lifecycle without stopping or restarting. The runner resumes and
tracks every unfinished future before awaiting promotion events, then commits
tool-result messages in original call order with their original provider call
ids; events may show actual completion order. A promoted result containing a
task id is only a running acknowledgement, so dependent work waits for the
separate terminal background message correlated by that id.

`delegate` starts a GeneralTask child in the background immediately.
`task_status`, `task_wait`, `task_inspect`, `task_steer`, and `task_stop`
provide explicit lifecycle operations. Inspect projects a bounded page of the
child's durable messages in their native Chat-compatible form. Steer appends a
durable pending ordinary user message after the child's current assistant
response and complete tool-call batch, before its next provider request. Stop
aborts the selected future and commits `cancelled`; it does not affect unrelated
tasks. Background work has no hard execution deadline.

Delegate context is explicit. A fresh child starts from its own initial
reminder and task. A fork child records the parent's pre-assistant message
sequence, materializes that entire prefix in its own Chat-compatible message
log, and then appends its child-specific reminder and task. Same-batch sibling
calls resolve to the same boundary. Copying the durable trajectory rather than
only the active projection preserves compaction/history behavior; run-local
pending-input ids are cleared. A complete child snapshot no longer reads the
parent on resume, while a partial snapshot may finish copying through its
already-recorded boundary.

Each task record is durable coordination state only. Child messages remain in
the child's run directory. Recovery derives delivered ids from the parent
transcript, marks in-flight ordinary tools `interrupted` with unknown side
effects, reconciles terminal children, and resumes queued/running children
through the same runner. Resume validates the frozen tool-schema hash before
task reconciliation can update any of those records.

The durable child guarantee belongs to `delegate` GeneralTask records, and the
parent run is the only resume entrypoint. Memory consolidation
uses this same path rather than a special direct-tool child.

## Prompt And Cache Shape

Agent and compaction calls use one invariant built-in system prompt and one
sorted, frozen tool-schema set. The history schemas are included from the first
call; automatic compaction never mutates this prefix. Project instructions,
skill metadata, memory paths, and delegated instructions form a deterministic
runtime reminder at the start of each run. The reminder is frozen for that run.
Optional startup schemas are selected before the run starts. Agent role and
remaining delegation depth change only the runtime-reminder tail, while a
compaction request changes only the message tail.

The durable trajectory remains append-only; before a normal model call, an
optional assistant compacted-state message can replace its older active prefix
while retaining the exact recent suffix. Large outputs become immutable
artifacts with bounded previews. These choices bound request growth while
keeping raw evidence inspectable and making provider KV-cache reuse possible
without making cache behavior part of the core API.

### Hooks

Command hooks observe `run_start`, `run_end`, `tool_before`, and `tool_after`.
They receive JSON over stdin and inherit the host process permissions. Hooks do
not define a second execution path.

## Headless Surface

The binary emits NDJSON runtime events for machines and a compact final result
for humans. There is no TUI or embedded web frontend. A future API or web client
should consume the same runtime events and run artifacts rather than introduce
model logic in the transport.

## Deliberate Launch Omissions

- OS sandbox and interactive approvals
- TUI or browser frontend
- built-in scheduler
- vector search
- provider/server-side compaction
- database-backed run indexing
- native dynamic plugins
- distributed subagents

These omissions reduce launch complexity. Existing boundaries allow them to be
added without creating another agent loop.
