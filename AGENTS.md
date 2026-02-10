# AGENTS.md

## Project Overview
picoagent is a minimal AI agent framework in TypeScript. Read README.md for architecture.

## Build & Test
- `npm run build` — compile TypeScript
- `npm test` — run all tests
- `npx tsc -p tsconfig.check.json` — strict type check (src + tests)

## Key Conventions

### Trust Boundaries (Zod vs Interface)
- **External data** (LLM tool args, API responses, config files) → validate with Zod schemas
- **Internal data** (messages, tool results, context) → plain TypeScript interfaces
- Never add Zod validation for internally-constructed objects

### Provider Abstraction
- core/loop.ts must NEVER import any SDK (anthropic, openai, etc.)
- All SDK-specific code lives in src/providers/
- Provider interface uses our own types (Message, ToolDefinition), not SDK types
- Tool[] → ToolDefinition[] conversion happens in loop.ts, not in providers

### Tool Implementation
- Tool parameters are Zod schemas, validated automatically by the agent loop
- Tools receive already-validated, typed args — no need to re-validate
- Use z.object().describe() for parameter descriptions (shows in JSON Schema for LLM)
- Keep tools focused: one tool = one capability

### Hooks System
- Use `AgentHooks` interface for lifecycle events
- `combineHooks()` merges multiple hook sets
- Built-in hook adapters: tracing (hooks/tracing.ts), compaction (hooks/compaction.ts), worker control (runtime/worker-control.ts)
- `onTextDelta` hook enables streaming (provider.stream() instead of provider.complete())

### Compaction
- `createCompactionHooks(provider, config)` — hook-based, agent loop has zero knowledge
- Fires on `onTurnEnd`, estimates tokens, summarizes when threshold exceeded
- Preserves recent messages, extracts file operations programmatically

### System Prompt Assembly
- `src/lib/prompt.ts` builds prompts from workspace files
- `buildMainPrompt(workspaceDir)`: SOUL.md → USER.md → AGENTS.md → memory.md → skill summaries → tool hints
- `buildWorkerPrompt(...)`: AGENTS.md → skill summaries → tool hints → protocol → constraints → task instructions (last)
- Files are optional — missing files are silently skipped

### Context Separation
| Context | Main Agent | Worker |
|---------|-----------|--------|
| SOUL.md | ✅ | ❌ |
| USER.md | ✅ | ❌ |
| AGENTS.md | ✅ | ✅ |
| memory.md | ✅ | ❌ |
| Skill summaries | ✅ | ✅ |
| task.md | ❌ | ✅ (own task) |
| write_file | unrestricted | task dir only |

### Worker Write Boundary
- Workers can read the entire workspace but can only write within their task directory (.tasks/t_xxx/)
- Enforced via `writeRoot` in ToolContext — write_file checks `path.startsWith(writeRoot)`
- Shell is soft-constrained: cwd set to task dir, system prompt instructs write-only-here
- Main Agent has no write restrictions

### File Organization
- src/core/ — kernel (loop, hooks, provider, types) — 4 files, frozen after v1
- src/runtime/ — orchestration (runtime, worker, worker-control)
- src/hooks/ — composable hook adapters (tracing, compaction)
- src/lib/ — shared utilities (frontmatter, prompt, task, tracer)
- src/providers/ — SDK-specific implementations
- src/tools/ — LLM-facing tool interfaces
- tests/ — mirrors src/ structure

### Scanning & Frontmatter
- Use `src/lib/frontmatter.ts` for all markdown scanning
- Frontmatter parser is custom (no external YAML lib) to keep core small
- Supports only: `key: value` (string/number/bool) and inline arrays `key: [a, b]`
- No nested objects or multi-line values
- Always use `---` delimiters

### Common Mistakes
- Importing SDK types in core/ files (breaks provider abstraction)
- Forgetting to handle the `toolResult` grouping in Anthropic (consecutive tool results must be in one user message)
- Using `Record<string, any>` instead of `Record<string, unknown>` (prefer unknown for type safety)
- Not running `npx tsc -p tsconfig.check.json` after changes
- Adding node_modules/ or dist/ to git (check .gitignore)
- Zod v4 uses `z.toJSONSchema()` not the old `zodToJsonSchema` package

### Testing
- Use Node built-in test runner (node:test)
- Mock the Provider interface for agent-loop tests
- Test at trust boundaries: tool args validation, API response parsing
- `tsconfig.check.json` includes both src/ and tests/ under strict mode

## Task Management

Tasks are isolated units of work with their own directory and state.

### Directory Structure
Each task lives in `.tasks/{id}/`:
- `task.md`: Definition + metadata in frontmatter
- `progress.md`: Worker's activity log
- `result.md`: Final output

### Lifecycle
Status flow: `pending` → `running` → `completed` | `failed` | `aborted`

### Core Tools
- `dispatch(name, description, instructions)`: Create a new task and spawn worker
- `steer(id, message)`: Redirect a running worker
- `abort(id)`: Cancel a running worker

## Workers & Runtime

### Worker Implementation
- One worker per task directory
- Reads instructions from `task.md`
- cwd and writeRoot scoped to task directory
- System prompt includes AGENTS.md + skills + protocol + constraints + task instructions
- NO user profile (SOUL.md, USER.md) or memories

### Runtime Coordination
- `src/runtime/runtime.ts` manages message routing + worker lifecycle
- Spawns workers via `spawnWorker(taskDir)`
- Tracks active workers via `Map<taskId, WorkerControl>`
- Injects completion notifications into main agent
- Worker control via hooks: abort flag + steer message queue
