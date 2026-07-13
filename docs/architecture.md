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
updates, and background control share the same registry. The registry is sorted so tool schema
order remains deterministic across requests.

This registry is the capability router: it maps a model-returned tool name to
one implementation and one schema. It does not decide what to do or create a
second planning layer; the model selects a capability, and the runner performs
the deterministic lookup. Duplicate names fail during startup instead of
silently replacing an existing capability.

### Run storage

Each run is a portable directory beneath `<workspace>/.pico/runs/<run-id>/`.
It contains run metadata, complete messages, structured events, the final answer,
artifacts, and background task records. This is what persistence means in the
launch runtime: a cloud worker can retain or inspect a job without a database.

Only complete messages are resumable. Stream deltas are emitted to sinks and may
be logged as events, but are not appended as partial conversation messages.

The persisted run state is intentionally coarse:

```text
queued -> running -> completed
                  `-> failed
```

The loop itself is a small state machine too: inject newly completed background
results, request model output, persist the complete assistant message, execute
zero or more direct tool calls, persist their results, then either repeat or
complete. This makes crash boundaries and event ordering explicit without
introducing a workflow engine.

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

### Skills and instructions

The stable system prefix contains built-in instructions, workspace `AGENTS.md`,
and sorted skill metadata. A skill body enters the conversation only after the
model calls `load_skill`.

### Memory

Memory is durable knowledge about the user and projects. The system prompt
exposes two ordinary Markdown locations; `read` and `bash` inspect them.
`memory_update` invokes the general-task profile to make semantic changes, and
an external cron or job scheduler can invoke model-driven consolidation. See
[memory.md](memory.md).

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

Stable instructions, project instructions, skill metadata, and sorted tool
schemas stay in deterministic order. Tool outputs and background completions
are append-only tail messages. Large outputs become immutable artifacts with
bounded previews. Together these choices avoid rebuilding earlier messages and
make provider KV-cache reuse possible without making cache behavior part of the
core API.

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
- database-backed run indexing
- native dynamic plugins
- distributed subagents

These omissions reduce launch complexity. Existing boundaries allow them to be
added without creating another agent loop.
