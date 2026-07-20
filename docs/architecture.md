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
        -> flat local Tool adapters
        -> MCP Tool adapters
        -> TaskManager
           -> explicitly spawned Tool
           -> child AgentRunner
     -> ArtifactStore
     -> RunDirStore
        -> compacted-state metadata / compacted-history reader
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

Every local model-facing adapter lives in a flat `src/tools/<tool>/` module.
Its compile-time Markdown description, name, schema, argument validation, and
execution adapter stay together. Domain engines remain separate: task state is
owned by `TaskManager`, skills by `SkillRegistry`, and trajectory retrieval by
`TrajectoryReader`. MCP lifecycle and its dynamic adapter remain in `mcp.rs`.

`build_app_tools` assembles process-wide local capabilities. `RunToolAssembly`
is the single path that adds run-scoped history and task controls. Every
registration explicitly says whether the model may name that tool in
`spawn(kind=tool)`; this does not affect automatic promotion of a direct call
whose foreground window elapses. The `spawn` tool's schema enum exposes the
exact allowed set, so the model-visible schema and resume hash commit the same
capability contract.

Root and delegating/leaf GeneralTask have explicit capability sets. Each normal
profile registers `history_search` and
`history_read` before its first call regardless of whether automatic compaction
is configured. A GeneralTask's delegating or leaf variant is selected from its
remaining depth before the run starts. Delegation depends on the selected
profile; optional `web_search` and MCP tools depend on startup configuration.
Memory paths do not add a tool schema. The selected schemas do not appear or
disappear during one run.

### Run storage

Each run is a portable directory beneath `<workspace>/.pico/runs/<run-id>/`.
It contains run metadata, append-only complete messages, structured events, the final answer, artifacts, and
background task records. This is what persistence means in the launch runtime:
a cloud worker can retain or inspect a job without a database.

The durable message contract is `openai-chat-compatible`. Every
`messages.jsonl` line is one Chat-shaped user, assistant, or tool message;
runtime reminder text is part of the first user message's `content`, not a
custom JSON variant. Assistant reasoning explicitly returned by a compatible
endpoint uses the optional `reasoning_content` extension. This extension is not
part of the official OpenAI Chat Completions schema.

Picoagent-only state lives in the paired `message_metadata.jsonl` sidecar. Each
line carries a run-local `m<N>` message id whose number equals its one-based
sequence, a timestamp, SHA-256 of the exact Chat JSON, the layout needed to
recover provider-neutral content, and a second SHA-256 over that reconstruction
metadata. A queued steering input's idempotency id is stored separately when
present. Tool-error state and opaque provider continuation items also remain
there, as do structured result artifact refs and the exact preview-byte counts
used to restore the per-run budget. Compaction request/state classification and
state boundaries also live only in the sidecar. None of these
private fields are added to the Chat message. `run.json` identifies the format
as `openai-chat-compatible`.

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
parent's committed `BackgroundTaskResult` messages; task JSON does not carry a
second authoritative `delivered` flag. A `spawn` result is one normal tool
result; later completion is a new user/runtime message, never a second tool
result with the same provider call id.

The full run holds a filesystem execution lease. Resume rebuilds the recorded
profile, validates provider/model/workspace identity, loads the paired message
log and latest completed compacted state, and continues after the last completed
model step. An unpaired direct tool request becomes an explicit interrupted
error result; it is never automatically replayed.

### Artifact storage

Large tool outputs are never discarded and do not enter the live context in
full. The store writes the complete bytes, records immutable metadata, and gives
the model a bounded beginning/end preview and a relative path it can inspect in
pages. See [artifacts.md](artifacts.md).

### Context compaction and trajectory retrieval

Local compaction changes the active-context projection without rewriting prior
messages. `messages.jsonl` retains every committed completed message with a
stable `m<N>` ref whose number is its sequence, including the successful
compaction user instruction and exact assistant compacted state. Sidecar
metadata marks those two records and stores the covered prefix and first exact
message kept. A normal active
request excludes compaction instructions and older compacted states; it contains
the initial runtime message, newest exact assistant state, and exact recent
ordinary suffix.

The trigger uses a provider-neutral request estimate from the first call and
adopts provider-reported input usage whenever available. Configuring
`compact_at_tokens` controls compacted-state creation only; the normal system
prompt and history-tool schemas are already present and remain unchanged. The
additional request uses the same provider, model, system, and frozen schemas,
then appends one compaction user instruction. Returned tool calls are rejected.
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

`spawn` starts a schema-listed tool or GeneralTask child in the background
immediately. A direct ordinary tool starts in the foreground; when its configured
foreground window elapses, the same in-flight future moves into this task
lifecycle without stopping or restarting. Explicit background work has no hard
execution deadline.

The `task` control surface provides status, bounded wait, inspect, steer, and
stop. Inspect projects a bounded page of the child's durable messages in their
native Chat-compatible form. Steer appends a durable pending ordinary user
message after the child's current assistant response and complete tool-call
batch, before its next provider request. Stop aborts the selected future and
commits `cancelled`; it does not affect unrelated tasks.

Each task record is durable coordination state only. Child messages remain in
the child's run directory. Recovery derives delivered ids from the parent
transcript, marks in-flight ordinary tools `interrupted` with unknown side
effects, reconciles terminal children, and resumes queued/running children
through the same runner. Resume validates the frozen tool-schema hash before
task reconciliation can update any of those records.

The durable child guarantee belongs to `spawn(kind="agent")` GeneralTask
records, and the parent run is the only resume entrypoint. Memory consolidation
uses this same path rather than a special direct-tool child.

## Prompt And Cache Shape

Agent and compaction calls use one invariant built-in system prompt and one
sorted, frozen tool-schema set. The history schemas are included from the first
call; automatic compaction never mutates this prefix. Project instructions,
skill metadata, memory paths, and delegated instructions form a deterministic
runtime reminder at the start of each run. The reminder is frozen for that run.
Optional schemas and a GeneralTask's delegating/leaf variant are selected before
the run starts. A compaction request changes only the message tail.

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
