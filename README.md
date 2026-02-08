# picoagent

A minimal, dual-agent framework for AI assistants. Small enough to understand, powerful enough to use.

## Why

Existing agent frameworks (OpenClaw, etc.) are powerful but complex — 50+ modules, deep abstraction layers, enterprise-grade config systems. For a single user who just wants a personal AI agent, most of that complexity is unnecessary.

**picoagent** strips the agent down to its essence:
- **~500 lines of core** that never need to change
- **File-based communication** between agents (no message queues, no event buses)
- **JSONL tracing** for full observability
- **Extensible via tools and skills**, not code changes

## Architecture

### Dual-Agent Design

```
┌─────────────────────────────────────────┐
│              Frontend Agent              │
│         (lightweight, fast model)        │
│                                          │
│  ✦ Always responsive — never blocks      │
│  ✦ Triages user messages                 │
│  ✦ Simple questions → answers directly   │
│  ✦ Complex tasks → dispatches to Backend │
│  ✦ Controls Backend via tools            │
│                                          │
│  Tools:                                  │
│    dispatch(task) — start a Backend task  │
│    steer(msg)     — redirect Backend      │
│    abort()        — cancel Backend task   │
│    read_file()    — read progress/status  │
├──────────────────────────────────────────┤
│         File System (the "bus")           │
│                                          │
│  .agent/                                 │
│  ├── control/                            │
│  │   ├── task.md     ← Frontend writes   │
│  │   └── signal      ← abort/steer       │
│  ├── state/                              │
│  │   ├── status.json ← machine-readable  │
│  │   └── progress.md ← human-readable    │
│  ├── result.md       ← final output      │
│  └── traces/                             │
│      └── {trace_id}.jsonl                │
├──────────────────────────────────────────┤
│              Backend Agent               │
│         (powerful model + tools)         │
│                                          │
│  ✦ Runs tool-calling loop                │
│  ✦ Updates progress.md after each step   │
│  ✦ Checks signal file between tools      │
│  ✦ Can spawn sub-agents for subtasks     │
│  ✦ Never talks to user directly          │
│                                          │
│  Tools:                                  │
│    shell(cmd)        — execute commands   │
│    read_file(path)   — read files         │
│    write_file(path)  — write files        │
│    + skill-provided tools                │
└──────────────────────────────────────────┘
```

### Frontend is Optional

Without Frontend, picoagent is a single-agent CLI tool (stdin → Backend → stdout).
With Frontend, it becomes a responsive assistant that handles multiple messages gracefully.

```
# Single-agent mode (no Frontend)
echo "refactor main.rs" | picoagent

# Dual-agent mode (with Frontend)
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
  Frontend: → dispatch → "On it!"   ← instant
  User: "also fix utils.rs"
  Frontend: → queue → "Queued"       ← instant
  User: "are you done yet?"
  Frontend: → reads progress.md      ← instant
```

The Frontend never blocks because it doesn't do heavy work.

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

### File-Based Communication

The file system is the message bus between Frontend and Backend.

| File | Writer | Reader | Purpose |
|------|--------|--------|---------|
| `control/task.md` | Frontend | Backend | Task instructions |
| `control/signal` | Frontend | Backend | Control signals (abort/steer) |
| `state/status.json` | Backend | Frontend | Machine-readable state |
| `state/progress.md` | Backend | Frontend | Human/LLM-readable progress |
| `result.md` | Backend | Frontend | Final output |

**Why files?**
- Zero infrastructure — no queues, no IPC, no sockets
- Naturally persistent — survives crashes
- Debuggable — `cat .agent/state/progress.md`
- The Backend already has file tools — no new capabilities needed

### Tracing

Every task gets a JSONL trace file for observability and benchmarking:

```
.agent/traces/{trace_id}.jsonl
```

Each line is a structured event:

```jsonl
{"ts":1707368520,"trace_id":"t_abc","span_id":"s_1","agent":"backend","event":"task_start","task":"refactor main.rs"}
{"ts":1707368521,"trace_id":"t_abc","span_id":"s_1","agent":"backend","event":"llm_call","model":"sonnet","tokens_in":1200}
{"ts":1707368523,"trace_id":"t_abc","span_id":"s_1","agent":"backend","event":"tool_start","tool":"read_file","args":{"path":"main.rs"}}
{"ts":1707368523,"trace_id":"t_abc","span_id":"s_1","agent":"backend","event":"tool_end","tool":"read_file","duration_ms":50}
{"ts":1707368525,"trace_id":"t_abc","span_id":"s_1","agent":"backend","event":"subagent_spawn","child_span":"s_2","task":"extract auth module"}
{"ts":1707368530,"trace_id":"t_abc","span_id":"s_2","agent":"subagent","event":"task_end","result":"done"}
{"ts":1707368535,"trace_id":"t_abc","span_id":"s_1","agent":"backend","event":"task_end","result":"refactoring complete"}
```

- `trace_id` correlates all events across agents (including sub-agents)
- `span_id` identifies individual agent runs
- Analyze with `jq`, build dashboards, or feed into benchmarking tools

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

Skills are modular extensions that provide domain knowledge + tools. Following the SKILL.md convention:

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
1. **Metadata** (name + description) — always in context (~100 tokens)
2. **SKILL.md body** — loaded when skill triggers
3. **References/scripts** — loaded on demand

The Core discovers skills by scanning the skills directory. Adding a skill = adding a folder. No code changes.

## Project Structure

```
src/
├── core/                  ← ~500 lines, stable
│   ├── agent-loop.ts      ← tool-calling loop
│   ├── llm.ts             ← LLM API calls
│   ├── loader.ts          ← tool/skill discovery
│   ├── trace.ts           ← JSONL tracing
│   ├── types.ts           ← Tool/Skill interfaces
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
- [ ] **v0.3** — Skill loading system
- [ ] **v0.4** — File-based progress tracking
- [ ] **v0.5** — Frontend agent (optional dual-agent mode)
- [ ] **v0.6** — Sub-agent support (Backend can fork child agents)
- [ ] **v0.7** — Channel integration (single channel, TBD)

## Acknowledgments

Inspired by studying the architectures of:
- [OpenClaw](https://github.com/openclaw/openclaw) — comprehensive agent framework
- [NanoClaw](https://github.com/gavrielc/nanoclaw) — minimal Claude agent with container isolation
- [pi-agent-core](https://github.com/nicobrinkkemper/pi-agent-core) — the agent loop that powers OpenClaw

## License

MIT
