# Long-Term Memory

Picoagent memory is durable knowledge accumulated across runs. It is not the
live context window, transcript persistence, or a run summary.

## Scopes And Files

- Global user memory: `$PICO_HOME/memory/user/`
- Project memory: `<workspace>/.pico/memory/project/`

Both locations contain ordinary human-editable Markdown. Picoagent does not
define a database schema, vector index, or dedicated memory read/search API.
The system prompt names the resolved paths; the model uses `read` for known
files and `bash` with `rg` for discovery.

Project rules that every agent must obey belong in `AGENTS.md`. Memory records
user preferences, historical explanations, prior decisions, and evolving
project knowledge. A stable rule may later be promoted into `AGENTS.md` through
an explicit repository change.

## Updates

`memory_update` is the only memory-specific model tool. Its arguments are a
scope and a semantic instruction. The tool forks the configured `general-task`
profile, restricts it to `read`, `write`, and `bash`, and asks it to inspect the
existing Markdown before making the smallest useful change. The child decides
whether information should be added, corrected, merged, or removed; Rust only
handles paths, execution, timeouts, and persistence.

A direct `memory_update` call is synchronous. The model can call
`spawn(kind="tool", tool="memory_update", ...)` when the update is independent
of the main task. Completion then arrives as a new background-result message and
does not block the main model loop.

Picoagent deliberately does not auto-record every successful run. That would
turn transcripts into noisy memory without model judgment.

## Consolidation

`pico memory consolidate` launches the same general-task profile with access to
the chosen memory directories. It performs semantic consolidation: it may
merge related facts, remove stale duplication, preserve provenance, and rewrite
the Markdown for clarity. The harness does no similarity scoring or domain
judgment.

Use an external cron, systemd timer, or cloud scheduler:

```cron
15 3 * * * /usr/local/bin/pico --workspace /workspace/project memory consolidate
```

Markdown remains the source of truth. A future index can be a rebuildable
projection, but vector search is not required for the launch runtime.

## Prompt And Persistence Behavior

Memory contents are not injected wholesale. Only the two paths and usage rules
are in the system prompt; selected reads and update summaries appear near the
conversation tail. Each maintenance child has its own run directory, transcript,
events, artifacts, and parent id, so memory changes remain auditable without
inflating the parent context.
