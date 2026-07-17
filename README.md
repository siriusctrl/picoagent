# picoagent

Picoagent is a small headless agent harness written in Rust. It is designed for
local automation and cloud jobs where another service owns the eventual web UI.

The launch runtime deliberately has one agent loop, one tool registry, portable
run directories, and no embedded TUI, database, scheduler, sandbox, or approval
system.

## Features

- OpenAI OAuth with device login, refresh, Codex auth import, and one 401 retry
- OpenAI-compatible Responses and Chat Completions streaming APIs
- Anthropic-compatible Messages streaming API
- streamed text, fragmented tool calls, and usage/cache-token fields in events
- compact `read`, `write`, and `bash` built-ins plus optional `web_search`
- exact-first, atomic multi-edit writes with CRLF/BOM preservation
- versioned artifact spill for large tool results with bounded head/tail previews
- run-level cumulative inline-result budget for stable context growth
- optional local context compaction with append-only checkpoints and read-only
  regex history retrieval
- Agent Skills discovery with progressive `SKILL.md` loading
- MCP stdio servers adapted into the same tool registry
- command hooks for run and tool lifecycle events
- synchronous tools plus generic `spawn`/`wait` background execution
- in-process general-task subagents that reuse the same runner
- ordinary Markdown user/project memory maintained with normal file tools
- self-contained run directories and optional NDJSON events

## Important Security Boundary

Picoagent does not provide a security sandbox in this release. Built-in tools,
MCP servers, hooks, and child agents run with the same permissions as the
picoagent process. Run it only in environments where that access is appropriate.

## Install

Picoagent requires Rust and `rg` (ripgrep). Artifact-backed compacted-history
search invokes `rg` directly and expects it on `PATH`.

```bash
cargo install --path .
```

The binary is named `pico`.

## Quick Start

Without a config file, picoagent uses a deterministic echo provider:

```bash
pico run "hello"
```

Run a task with machine-readable events:

```bash
pico run --output ndjson "inspect this repository and run its tests"
```

Runtime output is stored beneath the current project:

```text
.pico/runs/<run-id>/
  run.json
  messages.jsonl
  message_metadata.jsonl
  compactions.jsonl  # created after the first checkpoint
  events.jsonl
  final.md
  artifacts/
  tasks/
```

Inspect a previous result:

```bash
pico inspect <run-id>
```

Resume an interrupted or failed run from its last complete message:

```bash
pico resume <run-id>
```

Resume never replays a direct tool call whose result was not durably recorded.
It appends an error result saying the outcome and side effects are unknown, then
lets the model inspect the workspace before deciding what to do. Background
ordinary tools are likewise marked `interrupted`; child-agent runs keep their
own transcripts. Durable GeneralTask children created by `spawn` continue when
their parent is resumed; resume the parent run rather than invoking `pico
resume` on a child id directly.

## Prompt Layout

Picoagent keeps its built-in system prompt independent of the workspace and
unchanged across normal agent calls. At the start of each run, the first user
message contains a synthetic `<runtime-reminder>` block before the original
request. The reminder snapshots the workspace path, `AGENTS.md`, discovered
skill metadata, memory locations, and any delegated-task instructions that
apply to the profile. Compacted-history guidance appears only beside an actual
checkpoint, not in the initial reminder. Tool schemas are sorted, and both the
schemas and reminder are frozen for that run; configuration or file changes
take effect on the next run.

`messages.jsonl` uses the `openai-chat-compatible` format: each line is one
complete Chat message with ordinary `role` and `content` fields. The runtime
reminder is text at the beginning of the first user message, not a custom JSON
content type. Assistant tool calls use the Chat `tool_calls` shape, and tool
results use `role: "tool"`, `tool_call_id`, and `content`.

Local fields do not alter those messages. Stable `message_id`, sequence,
timestamp, exact-message and reconstruction-metadata hashes, internal content
layout, tool-error state, and any opaque provider continuation items are stored
in the paired `message_metadata.jsonl` sidecar. `run.json` declares
`"message_format": "openai-chat-compatible"` and retains the original user
prompt. The metadata line is written last and commits its corresponding message;
compaction never rewrites or deletes committed trajectory records. Its
checkpoints are appended separately to `compactions.jsonl` when enabled.

Stable agent instructions are folded scalar values in the typed, compile-time
`prompts/agents.yaml` registry. Standalone base tool descriptions remain
Markdown beside their Rust implementations under `src/tools/<tool>/`; names,
schemas, validation, and execution remain Rust contracts.

## Provider Setup

Configuration is loaded from the first existing path:

1. `--config <path>`
2. `<workspace>/.pico/config.toml`
3. `$HOME/.pico/config.toml`
4. built-in echo defaults

Config files are not merged, and unknown fields are rejected so misspelled
settings fail at startup.

### OpenAI OAuth

```toml
[provider]
kind = "openai-oauth"
model = "gpt-5.6-sol"
```

Authenticate once:

```bash
pico auth login
```

Credentials are stored at `$PICO_HOME/auth.json` (default
`$HOME/.pico/auth.json`). If no picoagent credentials exist, the provider can
import a compatible `$CODEX_HOME/auth.json` or `$HOME/.codex/auth.json`.

### OpenAI-compatible

```toml
[provider]
kind = "openai-compatible"
model = "my-model"
base_url = "http://127.0.0.1:8000/v1"
api_key = "${OPENAI_API_KEY}" # or a literal key
protocol = "chat-completions" # or "responses"
reasoning_effort = "medium" # optional; provider/model-specific
```

`api_key` accepts either a literal key or a whole environment reference such as
`${OPENAI_API_KEY}`. Keep a literal key in the user config at
`$HOME/.pico/config.toml` with restrictive file permissions rather than in a
workspace file that may be shared. If `api_key` is omitted, picoagent reads
`OPENAI_API_KEY`. The removed OpenAI-compatible `api_key_env` field is rejected;
write `api_key = "${OPENAI_API_KEY}"` instead.

`reasoning_effort` is optional. Picoagent sends it as `reasoning_effort` for
Chat Completions and as `reasoning.effort` for Responses. If omitted, the
provider's model default is used. Common values include `none`, `minimal`,
`low`, `medium`, `high`, and `xhigh`; accepted values depend on the endpoint and
model.

When Chat Completions reasoning is configured, `max_output_tokens` is sent as
`max_completion_tokens`, as required by reasoning-capable Chat endpoints.

The OpenAI-compatible adapter retries an initial HTTP 429 up to three times
with bounded exponential backoff. Parent and child requests also share the
runtime model-concurrency limit described below.

If a Chat Completions stream explicitly returns `delta.reasoning_content`,
picoagent stores it in the assistant message's optional `reasoning_content`
field in `messages.jsonl` and
emits transient `model_reasoning_delta` events to live event sinks. Per-chunk
text and reasoning events are not written to `events.jsonl`; the complete
assistant message is the durable trajectory. Reasoning token counts are
preserved in persisted `model_completed` events when the provider reports them.
Reasoning is kept out of the visible answer and `final.md`. This records only
reasoning text exposed by the compatible endpoint; it cannot recover reasoning
that the provider does not return.

Inspect the persisted reasoning for a run with:

```bash
jq -c 'select(.role == "assistant" and has("reasoning_content")) | {role, reasoning_content}' .pico/runs/<run-id>/messages.jsonl
jq -c '.' .pico/runs/<run-id>/events.jsonl
```

`reasoning_content` is an OpenAI-compatible endpoint extension, not an official
OpenAI Chat Completions message field. Picoagent writes it only when the
endpoint explicitly returns reasoning text, and replays it as the same separate
field on later requests to that compatible Chat endpoint.

### Anthropic-compatible

```toml
[provider]
kind = "anthropic-compatible"
model = "my-model"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
```

See [configuration.md](docs/configuration.md) for runtime, compaction, artifact,
MCP, hook, and memory settings.

## Context Compaction And History

Automatic local checkpoint creation is disabled by default. Enable it with a
threshold appropriate to the model's context window:

```toml
[compaction]
trigger_tokens = 100000
keep_recent_tokens = 20000
summary_max_output_tokens = 4096
history_search_max_matches = 50
```

After a provider reports input-token usage, picoagent tracks the active context
and can make an additional model call when it reaches `trigger_tokens`. That
call summarizes the older completed-message prefix. Later requests contain the
checkpoint summary plus the exact recent suffix, while `messages.jsonl` remains
append-only. Providers that do not report input-token usage do not trigger
automatic compaction. Start/completion/failure records remain in `events.jsonl`.
Compatible Chat `reasoning_content` and replayable opaque provider items are
included in the between-call estimate.

The normal agent receives the `history_search` and `history_read` schemas from
its first provider request whether or not `trigger_tokens` is configured.
Changing the threshold controls checkpoint creation only; it does not change
the normal system prompt or tool schemas. Before anything has been compacted,
the history tools simply have no compacted prefix to search or read.

Two read-only tools recover exact details omitted from the active request:

- `history_search({"pattern":"..."})` applies a Rust regular expression only
  to compacted messages and their linked textual tool-result artifacts. Results
  are newest-first and carry stable message refs.
- `history_read({"ref":"msg_...","before":2,"after":2})` reads a bounded
  conversation-ordered window around one ref and keeps tool calls paired with
  their results.

The local reader invokes `rg` for bounded-memory searches inside full textual
artifacts, so ripgrep must be available on `PATH` for that part of
`history_search`. A future remote reader can provide the same contract from a
database or service.

Neither tool has a cursor. `history_search_max_matches` limits a query to its
newest matches; if reached, older matches are omitted and the model must refine
the regex. This is distinct from artifact preview truncation: if the bounded
JSON/JSONL tool result is too large, its complete returned content is saved as
an artifact and can be inspected with `read` or `bash`/`rg`. Query-limit
omissions are not present in that artifact.

Each assembled agent profile has a sorted, frozen toolset. A profile compacts only
when both history tools and at least one artifact inspection tool (`read` or
`bash`) are present. Checkpoint summaries use a separate, tool-free request
profile rather than changing a normal agent registry.

This release implements local model-generated checkpoints only. It does not use
OpenAI or another provider's server-side compaction API.

## Large Tool Results

Small results are returned inline. Large results are written in full under the
current run and replaced in model context with:

- beginning and ending previews
- byte length and media type
- SHA-256 digest
- stable project-relative path

The model can inspect the complete output with bounded `read` calls or `bash`
plus `rg`, avoiding repeated commands and unnecessary context growth. See
[artifacts.md](docs/artifacts.md).

## Tools And Background Work

The launch built-ins are intentionally small:

- `read`: bounded UTF-8 reads for a known path
- `write`: full-file creation/replacement or an atomic list of targeted edits
- `bash`: local discovery, `rg`, tests, builds, and other Bash commands
- `history_search`: regex search over the compacted trajectory prefix
- `history_read`: a bounded message window around a returned history ref
- `web_search`: optional Brave-backed public web search

Root and depth-eligible GeneralTask delegation capabilities are selected before
the run starts; a leaf GeneralTask has no delegation tools. Memory adds paths to
the reminder, not a tool schema. `web_search` and MCP tools depend on startup
configuration. The resulting schemas are sorted and frozen before the run's
first normal provider call.

`write` requires every edit target to identify one non-overlapping region in
the original file. It tries exact matching first, then a conservative whole-line
indentation normalization. It does not use broad fuzzy similarity that could
silently modify the wrong code.

Every direct tool call is synchronous. `spawn` is the single asynchronous
wrapper: it can start either an existing tool or the `general-task` agent
profile and immediately returns a task id. `wait` is a bounded join. Completed
background results are appended as new runtime messages at the next model
boundary, which preserves provider tool-call validity and keeps prior prompt
prefixes reusable.

## Skills

Picoagent discovers Agent Skills from lowest to highest precedence:

1. `$HOME/.agents/skills/*/SKILL.md`
2. `<workspace>/.agents/skills/*/SKILL.md`
3. `<workspace>/skills/*/SKILL.md`

Only skill name and description enter the stable prompt prefix. `load_skill`
returns the instruction body without repeating that metadata, plus the absolute
`SKILL.md` and skill-directory paths needed to resolve referenced files.

```bash
pico skills list
```

## Subagents

`spawn` with `kind = "agent"` starts the configured `general-task` profile. A
child is another invocation of the same runner, not a second agent class. Each
child:

- invokes the same `AgentRunner` and provider
- has a separate run id, transcript, events, and artifacts
- records its parent run id
- shares the working project, so it can inspect and modify the same files
- receives its own model/step/output budget profile
- cannot spawn another child at the default depth limit

Parent and child model requests share `runtime.max_parallel_model_calls`, which
defaults to one for compatibility with rate-limited endpoints. Background tool
capacity remains independently controlled by `runtime.max_parallel_tasks`.
`runtime.model_request_timeout_seconds` (default 300) prevents a stalled model
request from holding a run or the shared model slot forever.

Only child results return to the parent context; full child transcripts remain
in their own run directories. The parent stores only coordination state under
`tasks/`. On parent resume, terminal-result delivery is derived from the parent
transcript, while queued/running child runs continue from their own last
complete messages. This recovery guarantee applies to every durable GeneralTask
task record, including one used for a large memory update.

## Long-Term Memory

Memory is durable knowledge about the user and projects, not the current
conversation. It is ordinary Markdown at two locations which are included in
an ordinary agent's initial runtime reminder:

- `$PICO_HOME/memory/user/` for cross-project user knowledge
- `<workspace>/.pico/memory/project/` for project-specific knowledge

There are no special memory tools. The model uses `read`, `write`, and `bash`
for small focused changes. For a large independent update, it can spawn an
ordinary `general-task` child, continue useful work, and reconcile the child
result before finishing.

```bash
pico memory consolidate
```

Use an external scheduler instead of embedding cron into the harness:

```cron
15 3 * * * /usr/local/bin/pico --workspace /workspace/project memory consolidate
```

See [memory.md](docs/memory.md).

## Architecture

```text
CLI/job
  -> AgentRunner
     -> ModelProvider
     -> ToolRegistry
        -> built-in tools
        -> MCP tools
        -> load_skill
        -> spawn / wait
           -> background Tool
           -> child AgentRunner
     -> ArtifactStore
     -> RunDirStore
     -> EventSink
```

Provider wire formats never enter the loop. MCP tools use the same `Tool`
contract as built-ins. Subagents use the same runner. Large results use the same
artifact contract regardless of source.

Read [architecture.md](docs/architecture.md) and
[design-choices.md](docs/design-choices.md) for the detailed boundaries and
tradeoffs.

## Development

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Run a complete local smoke task:

```bash
tmp=$(mktemp -d)
target/debug/pico --workspace "$tmp" run --output ndjson "smoke"
find "$tmp/.pico/runs" -maxdepth 3 -type f -print
```
