# Design Choices

## One Rust Runner

Main tasks and subagents use one `AgentRunner`. Child runs differ only by parent
id, depth, task instructions, and their own persisted run directory.

Rejected: a separate orchestrator agent type or child-specific model loop. That
would duplicate tool, provider, streaming, and persistence behavior.

## Headless First

The runtime emits structured events and portable artifacts. It does not contain
a TUI or web frontend.

Rejected: preserving the legacy Bun/Hono/Ink surfaces. A future service can wrap
the Rust library and event stream without owning agent behavior.

## File-Based Runs

The launch runtime uses one self-contained directory per run instead of SQLite
or an event-sourced service. Complete messages and metadata are enough for
inspection and provide the boundary for a future bounded resume command; object
storage can archive the directory as a unit.

Revisit when cross-run queries, multi-worker ownership, or server-side pagination
become concrete requirements.

## Artifact-First Tool Output

Large results are preserved in full but represented in model context by a small
versioned envelope. This was chosen over destructive truncation and over placing
unbounded stdout in every subsequent model request.

## Markdown Memory

Memory is human-editable Markdown outside the transcript. Ordinary `read` and
`bash` capabilities inspect it; a focused general-task child performs semantic
updates and cron-friendly consolidation.

Rejected for launch: vector databases, automatic recording of every successful
run, Rust-side semantic heuristics, and making raw transcripts or artifacts
equivalent to curated memory.

## One Async Wrapper

Direct tool calls are synchronous. `spawn` can wrap any spawnable tool or the
general-task agent profile, while `wait` provides a bounded join. This keeps
execution policy out of individual tool schemas and avoids duplicate
`foo`/`foo_async` capabilities.

## Conservative File Mutation

`write` supports complete writes and atomic multi-region replacements. Targets
must be unique and non-overlapping in the original file. A conservative
whole-line indentation fallback handles formatting drift; broad fuzzy or
similarity matching is rejected because a plausible wrong edit is worse than a
clear retry request.

## Direct Host Execution

The launch runtime intentionally executes tools and hooks with the picoagent
process permissions.

Rejected for launch: a partial permission UI that could be mistaken for an OS
sandbox. The `Tool`/runner boundary remains available for a future real runtime
isolation layer.

## Provider Adapters Own Wire Details

The loop understands canonical messages and tool calls only. OAuth refresh,
provider headers, SSE event shapes, and prompt-cache hints stay in provider
modules.

## Stable Prompt Prefix

The built-in system prompt contains only product identity and stable operating
rules. Workspace `AGENTS.md`, skill metadata, memory paths, and delegated-task
instructions are snapshotted into a synthetic runtime reminder at the start of
each run. Tool descriptions remain in sorted tool schemas rather than being
duplicated in the system prompt. The tool registry and reminder are frozen for
the run; changes take effect on the next run instead of rewriting prior
messages.

Rejected for launch: hot-reloading project context or tool definitions inside a
run. Appending revisions would grow context, while replacing earlier messages
would break the durable transcript boundary and provider prefix-cache reuse.

## External Scheduling

Memory consolidation is a command. Cron, systemd, Kubernetes, or another job
platform decides when it runs.

Rejected: an embedded scheduler and daemon lifecycle in the launch harness.
