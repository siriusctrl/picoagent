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

### Tracing
- Tracer is optional in agent-loop — pass it for observability, omit for zero overhead
- Each trace is one JSONL file: ~/.picoagent/traces/{trace_id}.jsonl
- Use span_id and parent_span to reconstruct call trees
- Events: agent_start/end, llm_start/end, tool_start/end, error

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
