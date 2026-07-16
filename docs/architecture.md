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
        -> built-in Tool
        -> MCP Tool adapter
        -> skill / memory_update Tool
        -> spawn / wait
           -> background Tool
           -> child AgentRunner
     -> ArtifactStore
     -> RunDirStore
        -> compaction checkpoints / compacted-history reader
     -> EventSink
```

`AgentRunner` is the only model/tool loop. It calls a provider, executes the
returned tool calls, appends complete tool results, and repeats until the model
returns a final answer or the configured step limit is reached.

## Core Boundaries

### Model provider

`ModelProvider` translates the canonical message and tool shapes to one wire
protocol. OAuth, API keys, SSE parsing, provider errors, and provider-specific
cache hints stay behind this boundary.

Initial adapters:

- OpenAI OAuth
- OpenAI-compatible Responses or Chat Completions
- Anthropic-compatible Messages
- deterministic echo for tests and smoke runs

### Tool registry

Every model-callable action implements `Tool`. Built-ins, skills, MCP, memory
updates, and background control share the same registry. The registry is sorted
and frozen before the first normal provider call so tool schema order and
membership remain deterministic across requests.

This registry is the capability router: it maps a model-returned tool name to
one implementation and one schema. It does not decide what to do or create a
second planning layer; the model selects a capability, and the runner performs
the deterministic lookup. Duplicate names fail during startup instead of
silently replacing an existing capability.

Standalone base tools live in flat `src/tools/<tool>/` modules. Their
model-facing description is compile-time Markdown beside the implementation;
their name, input schema, argument validation, and execution stay together in
Rust. Tools coupled to task supervision, memory, skills, or MCP remain owned by
those subsystems and adapt into the same registry.

Root, delegating/leaf GeneralTask, and MemoryMaintenance have explicit
capability sets. Each normal profile registers `history_search` and
`history_read` before its first call regardless of whether automatic compaction
is configured. A GeneralTask's delegating or leaf variant is selected from its
remaining depth before the run starts. Memory and delegation tools depend on
the selected profile and configured memory; optional `web_search` and MCP tools
depend on startup configuration. The selected schemas do not appear or
disappear during one run.

### Run storage

Each run is a portable directory beneath `<workspace>/.pico/runs/<run-id>/`.
It contains run metadata, append-only complete messages, optional append-only
compaction checkpoints, structured events, the final answer, artifacts, and
background task records. This is what persistence means in the launch runtime:
a cloud worker can retain or inspect a job without a database.

Only complete messages are resumable. Stream deltas are emitted to live sinks
but omitted from the persisted `events.jsonl` and are never appended as partial
conversation messages.

Message and compaction loaders tolerate only a torn final JSONL record; a later
append removes that partial tail first. Malformed completed records still fail
loading so corruption is not silently skipped.

The persisted run state is intentionally coarse:

```text
queued -> running -> completed
                  `-> failed
```

The loop itself is a small state machine too: inject newly completed background
results, optionally checkpoint an old completed-message prefix, request model
output, persist the complete assistant message, execute zero or more direct
tool calls, persist their results, then either repeat or complete. This makes
crash boundaries and event ordering explicit without introducing a workflow
engine.

Background tasks have a separate persisted state:

```text
queued -> running -> completed
                  |-> failed
                  `-> timed_out
```

`delivered` is orthogonal metadata indicating whether a terminal result has
already entered the parent context. A `spawn` result is one normal tool result;
later completion is a new user/runtime message, never a second tool result with
the same provider call id.

### Artifact storage

Large tool outputs are never discarded and do not enter the live context in
full. The store writes the complete bytes, records immutable metadata, and gives
the model a bounded beginning/end preview and a relative path it can inspect in
pages. See [artifacts.md](artifacts.md).

### Context compaction and trajectory retrieval

Local compaction changes request assembly, not the durable trajectory.
`messages.jsonl` retains every completed message with a stable ref and sequence;
`compactions.jsonl` appends summary checkpoints that identify the covered
prefix and first exact message kept. The active request contains the initial
runtime message, the newest checkpoint summary, and the exact recent suffix.

The trigger is deliberately based on provider-reported usage. If a provider
does not report input tokens, automatic compaction does not run. Configuring
`trigger_tokens` controls checkpoint creation only; the normal system prompt and
history-tool schemas are already present and remain unchanged. A checkpoint
summary is produced through an additional tool-free request profile using the
same provider and model; picoagent does not implement provider/server-side
compaction.

`history_search` and `history_read` expose a read-only `TrajectoryReader`
boundary. The local implementation searches only messages outside the active
context, plus full textual artifacts linked to their tool results. Search uses
Rust regular expressions, returns newest matches up to a configured cap, and
has no cursor. Read accepts one stable message ref and a bounded before/after
window, expanding it when necessary to preserve tool-call/result pairs. A
future remote or database-backed trajectory can implement the same reader
without granting the model filesystem write access.

For linked local artifacts, the query opens sidecar metadata lazily in message
order, resolves the exact digest carried by the foreground or background
result envelope, verifies it with a bounded-memory stream, and invokes `rg`
with bounded output. It stops after the requested newest matches plus one,
avoiding whole-artifact heap loads and unnecessary older scans. A call id alone
is not treated as immutable identity when multiple sidecars share it.

The launch local message source still materializes one run's trajectory JSONL
per history query. Artifact contents remain streamed and bounded. If run sizes
outgrow this simple backend, an indexed local or remote `TrajectoryReader` can
replace it without changing the model-facing tools.

A normal profile compacts only when both history tools and at least one generic
artifact inspection tool (`read` or `bash`) remain available,
preserving exact recovery as part of the compaction contract.

### Skills and instructions

The system prompt contains only stable built-in instructions loaded from
compile-time Markdown under `prompts/agents/`. Rust owns prompt precedence,
section ordering, dynamic values, and the `runtime_reminder` envelope. The first
user message carries that reminder with compacted-history recovery guidance,
the workspace `AGENTS.md`, and sorted skill metadata, followed by the original
request. A skill body enters the conversation only after the model calls
`load_skill`.

### Memory

Memory is durable knowledge about the user and projects. An ordinary agent's
runtime reminder exposes two Markdown locations; `read` and `bash` inspect them.
`memory_update` invokes the focused MemoryMaintenance profile to make semantic
changes, and an external cron or job scheduler can invoke the same profile for
model-driven consolidation. See [memory.md](memory.md).

### Subagents

A subagent is a child invocation of the same `AgentRunner`. It has its own run
directory and transcript, a `parent_run_id`, and a depth. The launch runtime runs
children in-process, shares the parent workspace and base tools, and caps depth
at one. “Shared workspace” means parent and child operate on the same working
project files; it is not a special second workspace abstraction. Child
transcripts stay out of the parent context; only the bounded final result and
artifact reference return to the parent.

`spawn` is also the async wrapper for ordinary tools. Direct calls remain
synchronous. This keeps async policy out of every individual tool schema while
still letting the model parallelize independent work. `wait` is a bounded join;
all background executions have a separate hard timeout.

## Prompt And Cache Shape

Normal agent calls use one invariant built-in system prompt and one sorted,
frozen tool-schema set. The history schemas are included from the first call;
automatic compaction never mutates this prefix. Project instructions,
compacted-history guidance, skill metadata, memory paths, and delegated
instructions form a deterministic runtime reminder at the start of each run.
The reminder is frozen for that run. Optional schemas and a GeneralTask's
delegating/leaf variant are selected before the run starts; MemoryMaintenance
has its own narrow toolset. Summary calls intentionally use a separate
tool-free profile.

The durable trajectory remains append-only; before a normal model call, an
optional compaction checkpoint can replace its older active prefix with one
summary while retaining the exact recent suffix. Large outputs become immutable
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
