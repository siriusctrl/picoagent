# AGENTS.md

## Project Overview
picoagent is a minimal AI agent framework in TypeScript. Read README.md for architecture.

## Build & Test
- `npm run build` — compile TypeScript
- `npm test` — run all tests
- `npx tsc --noEmit` — type check without emitting

## Key Conventions

### Trust Boundaries (Zod vs Interface)
- **External data** (LLM tool args, API responses, config files) → validate with Zod schemas
- **Internal data** (messages, tool results, context) → plain TypeScript interfaces
- Never add Zod validation for internally-constructed objects

### Provider Abstraction
- agent-loop.ts must NEVER import any SDK (anthropic, openai, etc.)
- All SDK-specific code lives in src/providers/
- Provider interface uses our own types (Message, ToolDefinition), not SDK types
- Tool[] → ToolDefinition[] conversion happens in agent-loop, not in providers

### Tool Implementation
- Tool parameters are Zod schemas, validated automatically by the agent loop
- Tools receive already-validated, typed args — no need to re-validate
- Use z.object().describe() for parameter descriptions (shows in JSON Schema for LLM)
- Keep tools focused: one tool = one capability

### Hooks System
- Use `AgentHooks` interface for lifecycle events:
  - `onLoopStart`, `onLoopEnd`
  - `onLlmStart`, `onLlmEnd`
  - `onToolStart`, `onToolEnd`
  - `onTurnEnd`
  - `onError`
  - `onTextDelta` (for streaming)
- `combineHooks()` merges multiple hook sets
- `trace-hooks.ts` implements tracing via hooks
- `worker-control.ts` implements task steering/aborting via hooks
- `compaction.ts` implements message compaction via hooks

### Compaction
- Use `createCompactionHooks` to enable automatic message history compaction.
- Configured via `CompactionConfig` (context window, trigger ratio).
- Runs on `onTurnEnd`, summarizing history when threshold is reached.
- Preserves recent messages and extracts file operations into the summary.

### File Organization
- src/core/ — stable foundation, minimize changes
- src/providers/ — SDK-specific implementations
- src/tools/ — built-in tools
- tests/ — mirrors src/ structure

### Scanning & Frontmatter
- Use `src/core/scanner.ts` for all markdown scanning
- Frontmatter parser is custom (no external YAML lib) to keep core small
- Supports only: `key: value` (string/number/bool) and inline arrays `key: [a, b]`
- No nested objects or multi-line values
- Always use `---` delimiters

### Common Mistakes
- Importing SDK types in core/ files (breaks provider abstraction)
- Forgetting to handle the `toolResult` grouping in Anthropic (consecutive tool results must be in one user message)
- Using `Record<string, any>` instead of `Record<string, unknown>` (prefer unknown for type safety)
- Not running `npx tsc --noEmit` after changes
- Adding node_modules/ or dist/ to git (check .gitignore)
- Zod v4 uses `z.toJSONSchema()` not the old `zodToJsonSchema` package

### Testing
- Use Node built-in test runner (node:test)
- Mock the Provider interface for agent-loop tests
- Test at trust boundaries: tool args validation, API response parsing

## Task Management (v0.4)

Tasks are isolated units of work with their own directory, state, and tools.

### Directory Structure
Each task lives in `.tasks/{id}/`:
- `task.md`: Immutable definition (instructions) + mutable state (status) in frontmatter.
- `progress.md`: Worker's log of activity.

### Lifecycle
Status flow: `pending` → `running` → `completed` | `failed` | `aborted`

### Core Tools
- `dispatch(name, description, instructions)`: Create a new task (pending).
- `steer(id, message)`: Redirect a running task.
- `abort(id)`: Stop a running task.

## Workers & Runtime (v0.5)

### Worker Implementation
- One worker per task directory
- Runs `src/core/worker.ts` async
- Reads instructions from `task.md`
- Updates status: `pending` → `running` → `completed` | `failed`
- Writes logs to `progress.md`
- Writes final output to `result.md`
- System prompt includes task context but NO user profile or personality
- Uses blocking `runAgentLoop` internally with hooks for control and tracing

### Runtime Coordination
- `src/core/runtime.ts` manages the main loop
- Spawns workers via `spawnWorker(taskDir)`
- Tracks active workers in memory via `WorkerControl`
- Injects completion notifications into the main agent stream via `onUserMessage`
- Main agent wakes up to handle worker results

### Tool Context
- `onTaskCreated` callback added to `ToolContext`
- `onSteer` callback for `steer` tool
- `onAbort` callback for `abort` tool
- `dispatch` tool triggers `onTaskCreated` to spawn workers immediately
