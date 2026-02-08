# picoagent

A minimal agent framework for AI assistants. Small enough to understand, powerful enough to use.

## Why

Existing agent frameworks are powerful but complex — 50+ modules, deep abstraction layers, enterprise-grade config systems. For a single user who just wants a personal AI agent, most of that complexity is unnecessary.

**picoagent** strips the agent down to its essence:
- **~600 lines of core** that never need to change
- **File-based state** for tasks, memory, and progress
- **JSONL tracing** for full observability
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
│  ✦ Injects completion messages            │
│     (Worker done → wake Main Agent)       │
├──────────────────────────────────────────┤
│           Workers (async, ×N)             │
│                                           │
│  ✦ One Worker = one task directory         │
│  ✦ Runs tool-calling loop                 │
│  ✦ Updates progress.md after each step    │
│  ✦ Checks signal file between tools       │
│  ✦ Can spawn sync sub-workers             │
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

```typescript
// The entire runtime coordination
class Runtime {
  async onUserMessage(msg: string) {
    await this.mainAgent.prompt(msg);
  }

  async onWorkerComplete(taskId: string) {
    await this.mainAgent.prompt(
      `[Task ${taskId} completed. See .tasks/${taskId}/result.md]`
    );
  }

  async onWorkerError(taskId: string, error: string) {
    await this.mainAgent.prompt(
      `[Task ${taskId} failed: ${error}]`
    );
  }
}
```

## Core Concepts

### Agent Loop

The heart of picoagent — a simple tool-calling loop used by both Main Agent and Workers:

```
loop {
    response = call_llm(messages, tools)

    if response.is_text():
        return response

    for tool_call in response.tool_calls():
        result = execute_tool(tool_call)
        messages.push(tool_result(result))

        // Workers: check for interrupts
        if signal_file_changed():
            handle_signal()
            break

        // Workers: update progress
        update_progress_file()

    if pending_messages:
        messages.extend(pending)
}
```

One loop, shared by all agents. No steering vs follow-up distinction — one pending queue, the LLM decides how to handle new messages.

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
├── AGENTS.md                     ← agent behavior (has Main + Worker sections)
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

- Agents can modify workspace freely, but not runtime config
- config.yaml is hot-reloaded — change models without restart
- Crash recovery: read task status from frontmatter, resume

### Context Separation

Main Agent and Workers get different context:

| Context | Main Agent | Worker |
|---------|-----------|--------|
| SOUL.md | ✅ (persona) | ❌ |
| USER.md | ✅ (preferences) | ❌ |
| AGENTS.md | ✅ (Main section) | ✅ (Worker section) |
| memory.md | ✅ (always loaded) | Selected subset |
| Skill descriptions | ✅ (frontmatter, for routing) | ✅ (full body, for execution) |
| Task state | All active tasks | Own task only |

Main Agent curates what context each Worker receives at dispatch time — relevant memory, triggered skills, task instructions. Workers stay focused and token-efficient.

### Task Directory

One Worker = one task directory. Self-contained, scannable, archivable.

```
.tasks/t_001/
├── task.md          ← definition + metadata (frontmatter)
├── progress.md      ← live progress (the emit channel)
├── result.md        ← final deliverable
└── signal           ← ephemeral control (deleted after consumed)
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
trace_id: t_abc
---

## Instructions
(task details)

## Constraints
(boundaries)
```

**progress.md** — the emit channel. What Backend writes here, the user sees:

```markdown
## Plan
- [x] Read source file
- [x] Identify extractable functions
- [ ] Extract handleAuth → auth.rs
- [ ] Run tests

## Log
14:02 Read main.rs (2347 lines)
14:03 Found 5 extractable functions

## Decisions
- Skipping parseConfig: depends on globalState
```

**signal** — ephemeral. `abort` or `steer`. Consumed then deleted.

### Progressive Disclosure

**Everything scannable is a markdown file with YAML frontmatter.** One pattern for all retrieval:

```markdown
---
name: "some entity"
tags: [a, b]
---
(detailed content, loaded on demand)
```

Applies to: tasks, skills, topic memories.

**Two tools power all retrieval:**

```typescript
// scan: narrow candidates, returns frontmatter only
scan(dir, pattern?) → DocMeta[]

// load: read full content
load(path) → { frontmatter, body }
```

```
scan(".tasks/", { tags: "*refactor*" })   → 12 frontmatter entries
LLM picks relevant ones
load(".tasks/t_001/task.md")              → full content
```

Small + essential (memory.md, skill descriptions) → system prompt.
Large + accumulated (task history, topic memories) → scan/load.

### Smart Routing

Main Agent uses LLM intelligence to handle user messages:

```
# Clear target
User: "stop the refactoring"
Main: scan(.tasks/, {status:"running"}) → one refactor task → abort(t_001)

# Ambiguous
User: "stop the refactoring"
Main: scan → two refactor tasks
Main: "I have two refactoring tasks running:
       1. refactor main.rs (5/7 done)
       2. refactor auth module (2/4 done)
       Which one? Or both?"

# Simple question
User: "what time is it?"
Main: "3:48 PM" (no Worker needed)
```

### Three Levels of Observability

| File | Granularity | Audience | Content |
|------|------------|----------|---------|
| `progress.md` | Key milestones | User / Main Agent | `14:03 Found 5 extractable functions` |
| `traces/*.jsonl` | Every event | Machine / benchmark | `{"event":"tool_start","tool":"read_file"}` |
| `sessions/*.jsonl` | Full conversation | Developer debug | Raw LLM messages |

Traces include `trace_id`, `span_id`, and `parent_span` for call stack reconstruction across Workers and sub-workers.

### Tools

Core only knows the interface:

```typescript
interface Tool {
  name: string;
  description: string;
  params: JsonSchema;
  execute: (args: any, ctx: ToolContext) => Promise<string>;
}

interface ToolContext {
  workdir: string;
  traceId: string;
  spanId: string;
  signal: AbortSignal;
}
```

Workers get path-scoped tools: can access workspace freely, but cannot access other Workers' task directories.

### Skills

Modular extensions — domain knowledge + tools:

```
skills/{name}/
├── SKILL.md           ← frontmatter + instructions
├── scripts/           ← optional executables
├── references/        ← optional docs (loaded on demand)
└── assets/            ← optional resources
```

Three-level progressive loading:
1. **Frontmatter** — always in Main Agent's system prompt (~100 tokens each)
2. **SKILL.md body** — loaded when skill triggers
3. **References/scripts** — loaded on demand by the agent

## Project Structure

```
src/
├── core/                  ← ~600 lines, stable after v1
│   ├── agent-loop.ts      ← tool-calling loop (shared)
│   ├── runtime.ts         ← message routing + worker lifecycle
│   ├── llm.ts             ← LLM API calls
│   ├── scanner.ts         ← frontmatter scan/load (universal)
│   ├── trace.ts           ← JSONL tracing
│   └── types.ts           ← Tool/Skill/DocMeta interfaces
│
├── tools/                 ← built-in tools (extensible)
│   ├── shell.ts
│   ├── read-file.ts
│   ├── write-file.ts
│   ├── scan.ts
│   └── load.ts
│
├── skills/                ← skill packages (extensible)
│   └── .../
│
└── main.ts                ← entry point
```

**Design principle:** `core/` is frozen after v1. All new functionality = new tools or new skills.

## Roadmap

- [ ] **v0.1** — Single agent, stdin/stdout, 3 tools (shell, read, write)
- [ ] **v0.2** — Agent loop + JSONL tracing
- [ ] **v0.3** — Skill loading + scan/load
- [ ] **v0.4** — Task directories + progress tracking
- [ ] **v0.5** — Async Workers + Runtime notifications
- [ ] **v0.6** — Sync sub-workers (Worker can fork child workers)
- [ ] **v0.7** — Channel integration (single channel, TBD)

## Acknowledgments

Inspired by studying the architectures of:
- [OpenClaw](https://github.com/openclaw/openclaw) — comprehensive agent framework
- [NanoClaw](https://github.com/gavrielc/nanoclaw) — minimal Claude agent with container isolation

## License

MIT
