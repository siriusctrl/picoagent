# picoagent

A minimal, dual-agent framework for AI assistants. Small enough to understand, powerful enough to use.

## Why

Existing agent frameworks (OpenClaw, etc.) are powerful but complex — 50+ modules, deep abstraction layers, enterprise-grade config systems. For a single user who just wants a personal AI agent, most of that complexity is unnecessary.

**picoagent** strips the agent down to its essence:
- **~500 lines of core** that never need to change
- **File-based communication** between agents (no message queues, no event buses)
- **JSONL tracing** for full observability
- **Extensible via tools and skills**, not code changes
- **Everything is a markdown file with frontmatter** — one pattern for all discovery and retrieval

## Architecture

### Agent Hierarchy

picoagent has a unified agent model with two relationship types:

```
Frontend ──async──→ Backend₁ ──sync──→ Subagent
         ──async──→ Backend₂ ──sync──→ Subagent
         ──async──→ Backend₃
```

| Relationship | Caller | Worker | Sync | Result Delivery |
|-------------|--------|--------|------|-----------------|
| Frontend → Backend | Frontend | Backend | **async** (don't wait) | Files (progress.md / result.md) |
| Backend → Subagent | Backend | Subagent | **sync** (wait) | Return value |

- **Frontend** is the orchestrator — dispatches multiple Backends, routes user messages, handles ambiguity
- **Backend** is the worker — runs tool-calling loops, can spawn sync subagents for subtasks
- **Subagent** is a child worker — executes a focused subtask and returns the result

```
┌──────────────────────────────────────────┐
│              Frontend Agent               │
│         (lightweight, fast model)         │
│                                           │
│  ✦ Always responsive — never blocks       │
│  ✦ Dispatches multiple Backends in parallel│
│  ✦ Routes steer/abort to correct Backend  │
│  ✦ Disambiguates when needed              │
│  ✦ Simple questions → answers directly    │
│                                           │
│  Tools:                                   │
│    dispatch(task) — start a Backend task   │
│    steer(id, msg) — redirect a Backend    │
│    abort(id)      — cancel a Backend task  │
│    scan(dir)      — search by frontmatter  │
│    load(path)     — read full content      │
├──────────────────────────────────────────┤
│         File System (the "bus")            │
│                                           │
│  .agent/                                  │
│  ├── tasks/                               │
│  │   ├── t_001/        ← Backend₁ running │
│  │   ├── t_002/        ← Backend₂ running │
│  │   ├── t_003/        ← completed        │
│  │   Each task dir contains:              │
│  │     task.md       ← instructions       │
│  │     progress.md   ← live progress      │
│  │     result.md     ← final output       │
│  │     status.json   ← machine state      │
│  │     signal        ← control signal     │
│  ├── memory/                              │
│  │   ├── memory.md   ← core (in prompt)   │
│  │   └── {topic}.md  ← on-demand          │
│  └── traces/                              │
│      └── {trace_id}.jsonl                 │
├──────────────────────────────────────────┤
│            Backend Agent (×N)             │
│         (powerful model + tools)          │
│                                           │
│  ✦ Runs tool-calling loop                 │
│  ✦ Updates progress.md after each step    │
│  ✦ Checks signal file between tools       │
│  ✦ Can spawn sync sub-agents for subtasks │
│  ✦ Never talks to user directly           │
│                                           │
│  Tools:                                   │
│    shell(cmd)        — execute commands    │
│    read_file(path)   — read files          │
│    write_file(path)  — write files         │
│    scan(dir)         — search by metadata  │
│    load(path)        — read full content   │
│    + skill-provided tools                 │
└──────────────────────────────────────────┘
```

### Frontend is Optional

Without Frontend, picoagent is a single-agent CLI tool (stdin → Backend → stdout).
With Frontend, it becomes a responsive multi-task assistant.

```
# Single-agent mode (no Frontend)
echo "refactor main.rs" | picoagent

# Multi-agent mode (with Frontend)
picoagent --frontend
```

### Why Not Main/Subagent?

Existing frameworks use a main agent that spawns subagents. The problem: **the main agent blocks while thinking**.

```
Main/Subagent:
  User: "refactor main.rs"
  Main: [thinking... 30s...]         ← blocked
  User: "also fix utils.rs"          ← queued, no response
  User: "are you done yet?"          ← queued, no response
  Main: [done] → processes backlog   ← bad UX

Frontend/Backend:
  User: "refactor main.rs"
  Frontend: → dispatch(Backend₁) → "On it!"       ← instant
  User: "also fix utils.rs"
  Frontend: → dispatch(Backend₂) → "Started too!"  ← instant, parallel
  User: "are you done yet?"
  Frontend: → scan active tasks → "Backend₁ is 3/7 done, Backend₂ just started"
```

The Frontend never blocks because it doesn't do heavy work. And it can run **multiple Backends simultaneously**.

### Smart Routing & Disambiguation

The Frontend uses LLM intelligence to route messages to the right Backend:

```
# Clear target — route directly
User: "stop the refactoring"
Frontend: scan(tasks/, {status: "running"}) → only one refactor task → abort(t_001)

# Ambiguous — ask the user
User: "stop the refactoring"
Frontend: scan(tasks/, {status: "running"}) → two refactor tasks found
Frontend: "I have two refactoring tasks running:
           1. t_001: refactor main.rs (5/7 done)
           2. t_002: refactor auth module (2/4 done)
           Which one should I stop? Or both?"

# Unrelated to any Backend — answer directly
User: "what time is it?"
Frontend: "It's 3:48 PM"   ← no Backend involved
```

## Core Concepts

### Agent Loop

The heart of picoagent — a simple tool-calling loop:

```
loop {
    response = call_llm(messages, tools)

    if response.is_text():
        return response    // done

    for tool_call in response.tool_calls():
        result = execute_tool(tool_call)
        messages.push(tool_result(result))

        // check for interrupts
        if signal_file_changed():
            handle_signal()
            break

        update_progress_file()

    // drain pending messages (if any)
    if pending_messages:
        messages.extend(pending)
}
```

No steering vs follow-up distinction. One pending queue, consumed at two points:
1. Between tool executions (can interrupt)
2. After inner loop completes (can extend)

The LLM decides how to handle new messages — not hardcoded policy.

### Workspace / Core Separation

Agent core state and workspace are fully separated:

```
~/.picoagent/                     ← core (agent doesn't touch directly)
├── config.yaml                   ← global config (hot reload)
├── tasks/                        ← task state
│   └── {task_id}/
│       ├── task.md               ← instructions + status (frontmatter)
│       ├── progress.md           ← live progress (user-facing)
│       ├── result.md             ← final output
│       ├── session.jsonl         ← LLM conversation history
│       └── signal                ← control signal (consumed then deleted)
├── traces/                       ← audit logs
│   └── {trace_id}.jsonl
└── agents/                       ← agent runtime
    └── frontend/
        └── session.jsonl

~/workspace/                      ← workspace (agent reads/writes freely)
├── AGENTS.md                     ← behavior instructions
├── SOUL.md                       ← persona
├── memory/
│   ├── memory.md                 ← core memory (injected into system prompt)
│   └── {topic}.md                ← topic memories (scan/load on demand)
├── skills/
│   └── {skill-name}/SKILL.md
└── (user's project files)
```

**Benefits:**
- Agent can freely modify workspace (memory, skills) but not its own runtime config
- Task state lives in core — crash recovery by reading task.md status
- config.yaml is hot-reloaded — change models/params without restart
- Workspace can be any directory; multiple agents can share one

### Task Directory

Each task is a self-contained directory. One Backend = one task directory.

```
tasks/t_001/
├── task.md          ← definition + metadata
├── progress.md      ← live progress (user-facing emit channel)
├── result.md        ← final deliverable
├── session.jsonl    ← full LLM conversation history
└── signal           ← ephemeral control (deleted after consumed)
```

**task.md** — definition + status (frontmatter follows the universal pattern):

```yaml
---
id: t_001
name: "refactor main.rs"
description: "Extract repeated functions into separate modules while keeping API stable"
status: running          # pending → running → completed / failed / aborted
created: 2024-02-08T14:02:00Z
started: 2024-02-08T14:02:01Z
completed: null
model: claude-sonnet
tags: [refactoring, backend]
trace_id: t_abc
---

## Instructions
Refactor main.rs, extract duplicated functions into independent modules.

## Constraints
- Do not change the public API
- Ensure all tests pass
```

Frontend writes the instructions + constraints. Backend only updates status fields in frontmatter.

**progress.md** — the emit channel:

progress.md IS the event stream. Backend writes here knowing the user will see it. Frontend watches the file for changes and forwards new content to the user.

```markdown
## Plan
- [x] Read source file
- [x] Identify extractable functions
- [ ] Extract handleAuth → auth.rs
- [ ] Run tests

## Log
14:02 Read main.rs (2347 lines)
14:03 Found 5 extractable functions
14:03 Extracting handleAuth (3 call sites to update)

## Decisions
- Skipping parseConfig: depends on globalState, needs larger refactor
```

Backend's system prompt rule for progress tracking:

> After each significant step, update `progress.md`:
> 1. **Plan first** — start with a checkbox TODO list
> 2. **Check off as you go** — mark items `[x]` when completed
> 3. **Log key events** — append timestamped one-liners
> 4. **Record decisions** — explain WHY when you skip or change approach

**result.md** — final deliverable (written on completion):

```yaml
---
summary: "Refactored main.rs, extracted 4 functions into modules"
files_changed: 5
tokens_used: 15000
duration_s: 45
---
(detailed results)
```

**signal** — ephemeral control. Contents: `abort` or `steer`. Consumed then deleted by Backend.

### Three Levels of Observability

| File | Granularity | Audience | Content |
|------|------------|----------|---------|
| `progress.md` | Key milestones | User / Frontend | `14:03 Found 5 extractable functions` |
| `traces/*.jsonl` | Every tool call, every LLM call | Machine / benchmark | `{"event":"tool_start","tool":"read_file"}` |
| `session.jsonl` | Full conversation | Developer debug | Raw messages array |

No duplication — each file serves a different consumer at a different granularity.

### Progressive Disclosure (Universal Pattern)

**Everything in picoagent is a markdown file with YAML frontmatter.** This creates a universal discovery protocol:

```markdown
---
name: "refactor main.rs"
status: completed
created: 2024-02-08T14:02:00Z
tags: [refactoring, backend]
---

(detailed content, loaded on demand)
```

The same pattern applies everywhere:

| Entity | Frontmatter Fields | Body |
|--------|-------------------|------|
| **Skill** | name, description | Usage instructions |
| **Task** | name, status, created, tags | Detailed plan + progress |
| **Memory** | topic, tags, updated, importance | Knowledge content |

**Two tools power all retrieval:**

```typescript
// scan: narrow candidates, returns frontmatter only
scan(dir: string, pattern?: Record<string, string>): DocMeta[]

// load: read full content of a specific file
load(path: string): { frontmatter, body }
```

`scan` supports pattern matching on frontmatter fields to reduce candidates:

```
scan("tasks/")                              → 200 frontmatter entries
scan("tasks/", { tags: "*refactor*" })      → 12 entries
scan("tasks/", { status: "completed" })     → 150 entries

# LLM reviews narrowed results, then selectively loads:
load("tasks/t_abc/task.md")                 → full content
```

**What goes where:**

| Content | Strategy | Reason |
|---------|----------|--------|
| `memory.md` (core) | System prompt | Always needed |
| Skill descriptions | System prompt | Always needed for triggering |
| Large task history | `scan` → `load` | Too many for context |
| Accumulated memories | `scan` → `load` | Grows over time |

Small + essential = inject into system prompt. Large + accumulated = scan/load on demand.

### Tracing

Every task gets a JSONL trace file for observability and benchmarking:

```
.agent/traces/{trace_id}.jsonl
```

Each line is a structured event with `parent_span` for call stack reconstruction:

```jsonl
{"ts":1707368520,"trace_id":"t_abc","span_id":"s_1","parent_span":null,"agent":"backend","event":"task_start","task":"refactor main.rs"}
{"ts":1707368521,"trace_id":"t_abc","span_id":"s_1","parent_span":null,"event":"llm_call","model":"sonnet","tokens_in":1200}
{"ts":1707368523,"trace_id":"t_abc","span_id":"s_1","parent_span":null,"event":"tool_start","tool":"read_file"}
{"ts":1707368525,"trace_id":"t_abc","span_id":"s_1","parent_span":null,"event":"subagent_spawn","child_span":"s_2","task":"extract auth module"}
{"ts":1707368525,"trace_id":"t_abc","span_id":"s_2","parent_span":"s_1","agent":"subagent","event":"task_start","task":"extract auth module"}
{"ts":1707368530,"trace_id":"t_abc","span_id":"s_2","parent_span":"s_1","event":"task_end","result":"done"}
{"ts":1707368535,"trace_id":"t_abc","span_id":"s_1","parent_span":null,"event":"task_end","result":"refactoring complete"}
```

Reconstruct the full call stack:

```
s_1: backend "refactor main.rs"
├── tool: read_file
├── tool: write_file
├── s_2: subagent "extract auth module"
│   ├── tool: read_file
│   └── tool: write_file
└── tool: shell("npm test")
```

### Tools

Tools are the interface between the agent and the world. The Core only knows the interface:

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

Built-in tools and skill-provided tools use the same interface. Adding a tool never requires changing the Core.

### Skills

Skills are modular extensions that provide domain knowledge + tools. They follow the same frontmatter convention as everything else:

```
skills/
├── web-search/
│   ├── SKILL.md           ← frontmatter (name + description) + instructions
│   ├── scripts/           ← optional executable scripts
│   ├── references/        ← optional docs (loaded on demand)
│   └── assets/            ← optional templates/resources
```

**SKILL.md example:**

```markdown
---
name: web-search
description: Search and fetch web content. Triggers on requests for real-time information, news, or documentation.
---

# Web Search

Use `search` tool for queries, `fetch` tool for page content.

## Tools
- `search(query)` — returns search results
- `fetch(url)` — extracts page content as markdown
```

**Three-level progressive loading:**
1. **Metadata** (name + description) — always in system prompt (~100 tokens)
2. **SKILL.md body** — loaded when skill triggers
3. **References/scripts** — loaded on demand by the agent

The Core discovers skills by scanning the skills directory. Adding a skill = adding a folder. No code changes.

## Project Structure

```
src/
├── core/                  ← ~500 lines, stable after v1
│   ├── agent-loop.ts      ← tool-calling loop
│   ├── llm.ts             ← LLM API calls
│   ├── scanner.ts         ← frontmatter scan/load (universal)
│   ├── trace.ts           ← JSONL tracing
│   ├── types.ts           ← Tool/Skill/DocMeta interfaces
│   └── bridge.ts          ← Frontend↔Backend file protocol
│
├── frontend/              ← optional
│   └── frontend.ts        ← triage + control tools
│
├── tools/                 ← built-in tools (extensible)
│   ├── shell.ts
│   ├── read-file.ts
│   └── write-file.ts
│
├── skills/                ← skill packages (extensible)
│   └── .../
│
└── main.ts                ← entry point
```

**Design principle:** `core/` is frozen after v1. All new functionality comes from adding tools or skills.

## Roadmap

- [ ] **v0.1** — Backend only, stdin/stdout, 3 tools (shell, read, write)
- [ ] **v0.2** — Tool-calling loop + JSONL tracing
- [ ] **v0.3** — Skill loading system + scan/load
- [ ] **v0.4** — File-based task tracking + memory
- [ ] **v0.5** — Frontend agent (optional dual-agent mode)
- [ ] **v0.6** — Sub-agent support (Backend can fork child agents)
- [ ] **v0.7** — Channel integration (single channel, TBD)

## Acknowledgments

Inspired by studying the architectures of:
- [OpenClaw](https://github.com/openclaw/openclaw) — comprehensive agent framework
- [NanoClaw](https://github.com/gavrielc/nanoclaw) — minimal Claude agent with container isolation

## License

MIT
