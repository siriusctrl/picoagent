# Fiasco MCP Artifact Format

Keep transport configuration separate from the model-facing artifact:

```toml
[mcp.github]
artifact = ".agents/mcp/github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp.github.env]
GITHUB_TOKEN = "${GITHUB_TOKEN}"
```

Relative artifact paths resolve from the Fiasco workspace. Environment values
use the same literal or `$NAME` / `${NAME}` rules as other Fiasco
configuration.

## Directory

```text
.agents/mcp/github/
├── MCP.md
├── catalog.json
└── references/
    ├── repository-research.md
    ├── issue-management.md
    └── pull-request-review.md
```

`MCP.md` and `catalog.json` are required. `references/` is optional for a very
small source, but normally carries the detailed progressive documentation.

## MCP.md

Use exactly `name` and `description` in frontmatter:

```markdown
---
name: github
description: Search GitHub repositories and manage issues and pull requests.
---

# GitHub

## Source map

- [Repository and code research](references/repository-research.md)
- [Issue management](references/issue-management.md)
- [Pull-request review](references/pull-request-review.md)
```

The name must match the `[mcp.<name>]` configuration key. Use at most 64
characters, begin with a lowercase letter, and use only lowercase letters,
digits, and single hyphens. Keep the description short enough for the runtime
catalog.

Every local Markdown link in `MCP.md` must stay inside the artifact and point
to an existing file. External HTTP links are allowed.

## catalog.json

Store the exact JSON array returned by MCP `tools/list`. Generate it only with:

```bash
fiasco --workspace <workspace> mcp capture <name>
```

The harness reads this file for exact remote tool names and input schemas. It
does not put the file in the model prompt. Do not add model summaries,
invocation examples, hashes, or validation state to it.

## Detailed References

Group highly related tools into capability documents rather than mirroring
the catalog one file at a time. A useful reference generally contains:

```markdown
# Repository Research

Use these commands to locate code and inspect the surrounding revision.

## Commands

### search_code

    search_code query:string repo?:string limit?:integer

    mcp("github search_code query='McpRuntime' repo=openai/fiasco limit=20")

### get_file_contents

    get_file_contents repo:string path:string ref?:string

    mcp("github get_file_contents repo=openai/fiasco path=src/mcp.rs")

## Workflow

Search first, then read the selected path and inspect commits only when the
task depends on revision history.
```

Keep signatures and examples consistent with the captured input schema.
Describe observed output shapes and pagination without copying large result
payloads into the artifact.

## Command Syntax

Use one grammar:

```text
<source> <tool> [name=value ...]
```

When a tool has exactly one input property, one positional value is also
accepted:

```text
github search_code 'McpRuntime'
```

Quote spaces with shell syntax:

```text
github search_code query='runtime tool registry'
```

The compiler preserves schema-declared strings and converts booleans, integers,
numbers, arrays, and objects according to their top-level property schemas:

```text
github search_code query=runtime limit=20 archived=false labels='["bug","runtime"]'
```

Use JSON only for an array or object value, not for the whole call.
