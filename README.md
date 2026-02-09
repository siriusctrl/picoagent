# picoagent

A minimal agent framework for AI assistants. Small enough to understand, powerful enough to use.

## Why

Existing agent frameworks are powerful but complex — 50+ modules, deep abstraction layers, enterprise-grade config systems. For a single user who just wants a personal AI agent, most of that complexity is unnecessary.

**picoagent** strips the agent down to its essence:
- **~950 lines of core** that stabilize after v1
- **File-based state** for tasks, memory, and progress
- **Hook-based observability** — tracing, worker control, streaming all via composable hooks
- **Extensible via tools and skills**, not code changes
- **Everything is a markdown file with frontmatter** — one pattern for all discovery and retrieval

## Architecture

### Main Agent + Async Workers

```
┌──────────────────────────────────────────┐
│              Main Agent                   │
│                                           │
│  ✦ Receives all user messages             │
│  ✦ Fast turns — routing + simple answers  │
│  ✦ Dispatches heavy work to Workers       │
│  ✦ Gets notified when Workers complete    │
│  ✦ Reads progress / relays results        │
│                                           │
│  Tools:                                   │
│    dispatch(task) — spawn async Worker     │
│    steer(id, msg) — redirect a Worker      │
│    abort(id)      — cancel a Worker        │
│    scan(dir)      — search by frontmatter  │
│    load(path)     — read full content      │
│    shell / read / write — simple tasks     │
├──────────────────────────────────────────┤
│               Runtime                     │
│                                           │
│  ✦ Routes user messages to Main Agent     │
│  ✦ Manages Worker lifecycle               │
│  ✦ Manages WorkerControl per task         │
│  ✦ Injects completion messages            │
│     (Worker done → wake Main Agent)       │
├──────────────────────────────────────────┤
│           Workers (async, ×N)             │
│                                           │
│  ✦ One Worker = one task directory         │
│  ✦ Runs tool-calling loop                 │
│  ✦ Updates progress.md after each step    │
│  ✦ Controlled via hooks (abort/steer)     │
│  ✦ Never talks to user directly           │
│  ✦ Completion triggers Main Agent wakeup  │
│                                           │
│  Tools:                                   │
│    shell / read / write                   │
│    scan / load                            │
│    + skill-provided tools                 │
└──────────────────────────────────────────┘
```

### How It Flows

```
User: "refactor main.rs"
  → Main Agent turn (fast):
    → dispatch(task) → "On it, refactoring main.rs"
    → Worker spawned async, Main Agent turn ends

User: "also fix the tests"
  → Main Agent turn (fast):
    → scan active tasks → Worker₁ is refactoring
    → dispatch(task) → "Started that too"
    → Worker₂ spawned async

User: "how's the refactoring going?"
  → Main Agent turn (fast):
    → load(.tasks/t_001/progress.md) → "3/7 steps done, extracting handleAuth now"

[Worker₁ completes → Runtime injects message → Main Agent wakes up]
  → Main Agent turn:
    → load(.tasks/t_001/result.md)
    → "Refactoring done! Changed 5 files, all tests pass ✅"
```

Main Agent stays fast because it never does heavy work directly. Heavy work goes to Workers. Workers notify Main Agent on completion via the Runtime.

### Hooks System

The agent loop supports a composable **hook system** for lifecycle observation and control. Hooks are optional — zero overhead when not provided.

```typescript
interface AgentHooks {
  onLoopStart?(): void | Promise<void>;
  onLoopEnd?(turns: number): void | Promise<void>;
  onLlmStart?(messages: Message[]): void | Promise<void>;
  onLlmEnd?(response: AssistantMessage, durationMs: number): void | Promise<void>;
  onToolStart?(call: ToolCall): void | Promise<void>;
  onToolEnd?(call: ToolCall, result: ToolResultMessage, durationMs: number):
    ToolResultMessage | void | Promise<ToolResultMessage | void>;
  onTurnEnd?(messages: Message[]): void | Promise<void>;
  onTextDelta?(text: string): void;
  onError?(error: Error): void | Promise<void>;
}
```

**Key timing:** `onToolEnd` fires after tool execution, before the result enters messages. `onTurnEnd` fires after all tool results are collected, before the next LLM call. This is where worker control (abort/steer) is checked.

Multiple hooks compose via `combineHooks()`:

```typescript
const hooks = combineHooks(
  createTraceHooks(tracer, "claude-sonnet"),   // observability
  createWorkerControlHooks(control, taskId),    // abort + steer
  { onTextDelta: (t) => process.stdout.write(t) } // streaming
);
```

Built-in hook adapters:
- **`createTraceHooks(tracer)`** — JSONL tracing via hooks (span tree reconstruction)
- **`createWorkerControlHooks(control, taskId)`** — abort flag + steer message queue

### Worker Control

Workers are controlled via an **in-memory message queue**, not file-based signals:

```typescript
class WorkerControl {
  abort(): void;           // set abort flag
  steer(msg: string): void; // push message to steer queue
}
```

- **Abort** — `onToolEnd` hook checks the abort flag; throws `AbortError` immediately
- **Steer** — `onTurnEnd` hook drains the steer queue and injects messages into the conversation before the next LLM call

The Runtime maintains a `Map<taskId, WorkerControl>`. The `steer` and `abort` tools operate on this map via context callbacks.

### Notification: Files for State, Runtime for Wakeup

Files store **what happened**. The Runtime handles **who gets notified**.

```
Worker completes:
  1. Writes result.md          ← file (persistent state)
  2. Updates task.md status    ← file (persistent state)
  3. Runtime injects message   ← runtime (triggers Main Agent)
     → Main Agent wakes up
     → Reads result.md
     → Tells user
```

## Core Concepts

### Agent Loop

The heart of picoagent — a unified tool-calling loop used by both Main Agent and Workers:

```
runAgentLoop(messages, tools, provider, context, systemPrompt?, hooks?) {
  hooks.onLoopStart()

  loop {
    hooks.onLlmStart(messages)
    response = provider.complete(messages, tools)  // or .stream() if onTextDelta hook
    hooks.onLlmEnd(response, duration)

    if no tool calls:
      hooks.onLoopEnd(turns)
      return response

    for each tool_call:
      hooks.onToolStart(tool_call)
      result = execute_tool(tool_call)
      result = hooks.onToolEnd(tool_call, result, duration) || result
      messages.push(result)

    hooks.onTurnEnd(messages)
  }
}
```

One function handles both streaming and non-streaming: if `hooks.onTextDelta` is set, the loop uses `provider.stream()` and emits text deltas. Otherwise it uses `provider.complete()`.

### Provider Abstraction

The agent loop never imports any SDK. All LLM-specific code lives behind the `Provider` interface:

```typescript
interface Provider {
  model: string;
  complete(messages, tools, systemPrompt?): Promise<AssistantMessage>;
  stream(messages, tools, systemPrompt?): AsyncIterable<StreamEvent>;
}
```

Currently implemented: `AnthropicProvider` (Claude). Adding OpenAI, Gemini, etc. = one new file in `src/providers/`.

### Trust Boundaries (Zod)

Zod validation at **trust boundaries only**:
- **Tool parameters** — LLM-generated, untrusted → Zod schema + `z.toJSONSchema()` for LLM
- **API responses** — external, untrusted → Zod validation
- **Internal types** — compiler-guaranteed → plain TypeScript interfaces

### Workspace / Core Separation

```
~/.picoagent/                     ← core (managed by runtime)
├── config.yaml                   ← global config (hot reload)
├── sessions/                     ← conversation history
│   ├── main.jsonl
│   └── workers/{task_id}.jsonl
└── traces/
    └── {trace_id}.jsonl

~/workspace/                      ← workspace (agents read/write freely)
├── AGENTS.md                     ← agent behavior
├── SOUL.md                       ← persona (Main Agent only)
├── USER.md                       ← user info (Main Agent only)
├── memory/
│   ├── memory.md                 ← core memory (Main Agent system prompt)
│   └── {topic}.md                ← topic memories (scan/load on demand)
├── skills/
│   └── {skill}/SKILL.md
└── .tasks/                       ← task directories
    ├── t_001/
    ├── t_002/
    └── ...
```

### Context Separation

Main Agent and Workers get different context:

| Context | Main Agent | Worker |
|---------|-----------|--------|
| SOUL.md | ✅ (persona) | ❌ |
| USER.md | ✅ (preferences) | ❌ |
| AGENTS.md | ✅ | ❌ |
| memory.md | ✅ (always loaded) | ❌ |
| Skill frontmatter | ✅ (for routing) | ✅ (same as Main) |
| Skill body | On demand | On demand |
| task.md | ❌ | ✅ (own task only) |
| Task state | All active tasks (scan) | Own task only |

Workers get **task.md + skills**. That's it. Main Agent curates everything the Worker needs into `task.md` at dispatch time.

### Task Directory

One Worker = one task directory. Self-contained, scannable, archivable.

```
.tasks/t_001/
├── task.md          ← definition + metadata (frontmatter)
├── progress.md      ← live progress (the emit channel)
└── result.md        ← final deliverable
```

**task.md** (frontmatter follows the universal pattern):

```yaml
---
id: t_001
name: "refactor main.rs"
description: "Extract repeated functions into separate modules"
status: running          # pending → running → completed / failed / aborted
created: 2024-02-08T14:02:00Z
started: 2024-02-08T14:02:01Z
completed: null
model: claude-sonnet
tags: [refactoring, backend]
---

## Instructions
(task details)

## Constraints
(boundaries)
```

Sequential IDs (`t_001`, `t_002`, ...) for human readability. Frontmatter round-trip preserved on status updates.

### Progressive Disclosure

**Everything scannable is a markdown file with YAML frontmatter.** One pattern for all retrieval:

```markdown
---
name: "some entity"
tags: [a, b]
---
(detailed content, loaded on demand)
```

**Two tools power all retrieval:**

```typescript
scan(dir, pattern?) → DocMeta[]   // frontmatter only, supports * wildcards
load(path)          → DocFull     // frontmatter + body
```

Small + essential (skill descriptions) → system prompt.
Large + accumulated (task history, topic memories) → scan/load.

### Three Levels of Observability

| File | Granularity | Audience | Content |
|------|------------|----------|---------|
| `progress.md` | Key milestones | User / Main Agent | `14:03 Found 5 extractable functions` |
| `traces/*.jsonl` | Every event | Machine / benchmark | `{"event":"tool_start","tool":"read_file"}` |
| `sessions/*.jsonl` | Full conversation | Developer debug | Raw LLM messages |

Traces include `trace_id`, `span_id`, and `parent_span` for call stack reconstruction across Workers and sub-workers.

### Compaction

Long sessions accumulate context. Compaction keeps agents within their context window.

**Layer 1: Tool Result Truncation** — head + tail pattern (key info often at end of output). Applied at tool execution time, before the result enters the session.

**Layer 2: Session Compaction** — summarizes old conversation history when context grows too large. Triggered when only 1/4 of context window remains.

Unified compaction prompt for Main Agent and Workers (3 fields: Goal, Key Decisions, Context). File operations extracted programmatically from tool call history.

## Project Structure

```
src/
├── core/                      ← ~950 lines, stable after v1
│   ├── agent-loop.ts          ← unified tool-calling loop (hooks-based)
│   ├── hooks.ts               ← AgentHooks interface + combineHooks
│   ├── provider.ts            ← Provider interface (SDK-agnostic)
│   ├── runtime.ts             ← message routing + worker lifecycle
│   ├── scanner.ts             ← frontmatter scan/load (universal)
│   ├── task.ts                ← task directory management
│   ├── trace.ts               ← JSONL tracer
│   ├── trace-hooks.ts         ← tracer → hooks adapter
│   ├── types.ts               ← Message/Tool/ToolContext interfaces
│   ├── worker.ts              ← worker execution
│   └── worker-control.ts      ← abort flag + steer queue + hooks
│
├── providers/                 ← SDK-specific implementations
│   └── anthropic.ts           ← Claude (streaming + non-streaming)
│
├── tools/                     ← built-in tools
│   ├── shell.ts               ← 30s timeout + output truncation
│   ├── read-file.ts
│   ├── write-file.ts
│   ├── scan.ts
│   ├── load.ts
│   ├── dispatch.ts            ← spawn async worker
│   ├── steer.ts               ← redirect a worker
│   └── abort.ts               ← cancel a worker
│
└── main.ts                    ← REPL entry point + runtime wiring

tests/                         ← mirrors src/ structure, strict mode
├── core/                      ← agent-loop, trace, scanner, task, worker, worker-control
├── tools/                     ← shell
├── helpers/                   ← shared mock provider
└── fixtures/                  ← test markdown files
```

**Design principle:** `core/` stabilizes after v1. All new functionality = new tools or new skills.

## Roadmap

- [x] **v0.1** — Provider abstraction, agent loop, 3 tools (shell, read, write), stdin/stdout REPL
- [x] **v0.2** — JSONL tracing (`trace_id` + `span_id` + `parent_span`)
- [x] **v0.3** — Frontmatter scanner + scan/load tools + skill discovery
- [x] **v0.4** — Task directories + dispatch/steer/abort tools
- [x] **v0.5** — Async Workers + Runtime
- [x] **v0.5.1** — Hook system (tracer, worker control, streaming as composable hooks)
- [x] **v0.6** — Compaction (hook-based, two-layer defense)
- [ ] TODO — Channel integration

## Stats

- **2241 lines** total (src + tests)
- **31 files** (21 src, 10 tests)
- **33 tests** (all passing, strict mode)

## Acknowledgments

Inspired by studying the architectures of:
- [OpenClaw](https://github.com/openclaw/openclaw) — comprehensive agent framework
- [NanoClaw](https://github.com/gavrielc/nanoclaw) — minimal Claude agent with container isolation

## License

MIT
