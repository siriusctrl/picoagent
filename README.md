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
- Agent Skills discovery with progressive `SKILL.md` loading
- MCP stdio servers adapted into the same tool registry
- command hooks for run and tool lifecycle events
- synchronous tools plus generic `spawn`/`wait` background execution
- in-process general-task subagents that reuse the same runner
- ordinary Markdown user/project memory maintained by a focused subagent
- self-contained run directories and optional NDJSON events

## Important Security Boundary

Picoagent does not provide a security sandbox in this release. Built-in tools,
MCP servers, hooks, and child agents run with the same permissions as the
picoagent process. Run it only in environments where that access is appropriate.

## Install

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
  events.jsonl
  final.md
  artifacts/
  tasks/
```

Inspect a previous result:

```bash
pico inspect <run-id>
```

## Provider Setup

Configuration is loaded from the first existing path:

1. `--config <path>`
2. `<workspace>/.pico/config.toml`
3. `$HOME/.pico/config.toml`
4. built-in echo defaults

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
workspace file that may be shared. The legacy `api_key_env = "OPENAI_API_KEY"`
form remains accepted for migration, but must not be combined with `api_key`.
If both fields are omitted, picoagent retains the legacy `OPENAI_API_KEY`
fallback.

`reasoning_effort` is optional. Picoagent sends it as `reasoning_effort` for
Chat Completions and as `reasoning.effort` for Responses. If omitted, the
provider's model default is used. Common values include `none`, `minimal`,
`low`, `medium`, `high`, and `xhigh`; accepted values depend on the endpoint and
model.

When Chat Completions reasoning is configured, `max_output_tokens` is sent as
`max_completion_tokens`, as required by reasoning-capable Chat endpoints.

If a Chat Completions stream explicitly returns `delta.reasoning_content`,
picoagent stores it as a separate `reasoning` block in `messages.jsonl` and
emits `model_reasoning_delta` events. Reasoning token counts are preserved in
`model_completed` events when the provider reports them. Reasoning is kept out
of the visible answer and `final.md`. This records only reasoning text exposed
by the compatible endpoint; it cannot recover reasoning that the provider does
not return.

Inspect the persisted reasoning for a run with:

```bash
jq -c '.content[]? | select(.type == "reasoning")' .pico/runs/<run-id>/messages.jsonl
jq -c 'select(.type == "model_reasoning_delta" or .type == "model_completed")' .pico/runs/<run-id>/events.jsonl
```

### Anthropic-compatible

```toml
[provider]
kind = "anthropic-compatible"
model = "my-model"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
```

See [configuration.md](docs/configuration.md) for runtime, artifact, MCP, hook,
and memory settings.

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
- `web_search`: optional Brave-backed public web search

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

Only skill name and description enter the stable prompt prefix. The full skill
body is loaded through `load_skill` when needed.

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

Only child results return to the parent context; full child transcripts remain
in their own run directories.

## Long-Term Memory

Memory is durable knowledge about the user and projects, not the current
conversation. It is ordinary Markdown at two locations which are included in
the system prompt:

- `$PICO_HOME/memory/user/` for cross-project user knowledge
- `<workspace>/.pico/memory/project/` for project-specific knowledge

The model uses `read` and `bash` to inspect memory—there are no special memory
search/read tools. `memory_update` delegates semantic editing to the configured
general-task model and returns its summary. Call it directly to wait, or wrap it
with `spawn` to let the update run in the background.

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
        -> skills and memory_update
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
