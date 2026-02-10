---
name: skill-creator
description: "Create or update skills and agent profiles. Use when designing, structuring, or packaging skills with scripts, references, and assets."
---

# Skill Creator

Guide for creating skills and agent profiles for picoagent.

## About Skills

Skills are self-contained packages in `workspace/skills/` that extend the agent with specialized knowledge, workflows, and tools. Agent profiles in `workspace/agents/` are LLM-powered skills that run as subagents.

### Skill Structure

```
skill-name/
├── SKILL.md (required)
│   ├── Frontmatter: name, description
│   └── Markdown body: instructions
├── scripts/        (optional) — executable code
├── references/     (optional) — docs loaded into context on demand
└── assets/         (optional) — files used in output (templates, etc.)
```

### Agent Profile Structure

```
agent-name.md (in agents/)
├── Frontmatter: name, description, model, provider, tags
└── Markdown body: system prompt additions
```

## Core Principles

### Concise is Key
The context window is shared. Only add what the model doesn't already know. Challenge each paragraph: "Does this justify its token cost?"

### Progressive Disclosure
1. **Frontmatter** (name + description) — always in context (~50 words)
2. **SKILL.md body** — loaded when skill triggers (<500 lines)
3. **References/scripts** — loaded on demand (unlimited)

### Degrees of Freedom
- **High freedom** (guidelines): Multiple approaches valid → text instructions
- **Medium freedom** (patterns): Preferred approach exists → pseudocode/examples
- **Low freedom** (procedures): Must be exact → scripts with few parameters

## Creating a Skill

### Step 1: Initialize

```bash
python3 scripts/init_skill.py <skill-name> --path workspace/skills [--resources scripts,references,assets]
```

### Step 2: Edit SKILL.md

**Frontmatter:**
- `name`: skill identifier (lowercase, hyphens)
- `description`: what it does AND when to trigger it. This is the primary discovery mechanism — be comprehensive.

**Body:** Instructions, workflows, examples. Keep under 500 lines. Split to references/ if longer.

### Step 3: Add Resources

- `scripts/` — deterministic code for repeated tasks (token efficient, reliable)
- `references/` — detailed docs loaded on demand (keeps SKILL.md lean)
- `assets/` — templates, images, files used in output (not loaded into context)

### Step 4: Validate

```bash
python3 scripts/validate_skill.py <path/to/skill-name>
```

## Creating an Agent Profile

Agent profiles are simpler — single markdown files in `agents/`:

```markdown
---
name: researcher
description: "Deep research — searches broadly, synthesizes findings"
model: gpt-4o
provider: openai
tags: [research]
---

(System prompt additions for this agent type)
```

Key fields:
- `model` + `provider` — which LLM to use (required for agents, not skills)
- Body becomes additional system prompt context for the worker
