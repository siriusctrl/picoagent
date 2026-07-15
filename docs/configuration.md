# Configuration

Picoagent reads TOML from an explicit `--config` path, the workspace
`.pico/config.toml`, or the user `$HOME/.pico/config.toml`. Workspace and user
files are alternatives in the launch runtime; they are not merged.

## Provider

Exactly one `[provider]` table is active.

```toml
[provider]
kind = "openai-oauth"
model = "gpt-5.6-sol"
# base_url = "https://chatgpt.com/backend-api/codex"
# auth_file = "/custom/pico-auth.json"
```

```toml
[provider]
kind = "openai-compatible"
model = "local-model"
base_url = "http://127.0.0.1:8000/v1"
api_key = "${OPENAI_API_KEY}" # or a literal key
protocol = "responses" # or "chat-completions"
reasoning_effort = "medium" # optional; provider/model-specific
```

`api_key` accepts either a literal value or a whole environment reference such
as `${OPENAI_API_KEY}`. Environment references are resolved when the runtime is
assembled. Keep literal credentials in `$HOME/.pico/config.toml` with
restrictive file permissions rather than in a workspace config that may be
shared. The legacy `api_key_env = "OPENAI_API_KEY"` form is still accepted for
migration. Configuring both `api_key` and `api_key_env` is an error.
If both are omitted, the runtime retains the legacy behavior of reading
`OPENAI_API_KEY`.

`reasoning_effort` is passed through as a string because OpenAI-compatible
providers and models support different levels. Picoagent maps it to
`reasoning.effort` for the Responses protocol and to the top-level
`reasoning_effort` field for Chat Completions. Omitting it preserves the
provider's default. Common values are `none`, `minimal`, `low`, `medium`,
`high`, and `xhigh`; some endpoints support additional values.

For Chat Completions, setting `reasoning_effort` also makes the runtime map
`max_output_tokens` to `max_completion_tokens`. Without reasoning configured,
the existing `max_tokens` request field is preserved for compatibility with
older OpenAI-compatible endpoints.

For compatible Chat streams, `delta.reasoning_content` is captured separately
from `delta.content`, persisted as `"type": "reasoning"` message content, and
excluded from subsequent conversation context. This follows Qwen's multi-turn
contract and keeps trajectory data separate from the visible assistant answer.
Empty deltas are ignored. If usage includes
`completion_tokens_details.reasoning_tokens`, that count is written to the
`model_completed` event. Responses usage reports the equivalent count under
`output_tokens_details.reasoning_tokens`.

This behavior only records fields the provider actually sends. OpenAI
Responses reasoning continuation items remain provider-owned items; the
runtime does not infer or expose hidden chain-of-thought.

```toml
[provider]
kind = "anthropic-compatible"
model = "claude-compatible-model"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
# anthropic_version = "2023-06-01"
```

## Runtime

```toml
[runtime]
max_steps = 32
max_subagent_depth = 1
max_parallel_tasks = 4
max_output_tokens = 8192
```

`max_steps` counts model calls, not individual tool calls. Child runs receive
their own step budget.

## Compaction And History Retrieval

```toml
[compaction]
# trigger_tokens = 100000       # omitted by default: automatic compaction off
keep_recent_tokens = 20000
summary_max_output_tokens = 4096
history_search_max_matches = 50
```

`trigger_tokens` enables local compaction and must be greater than zero. The
summary and history-search limits must also be positive. The trigger depends on
the active provider reporting input-token usage; a provider that omits it
cannot trigger automatic compaction. When the tracked context reaches the
threshold, picoagent uses the same provider and model for an additional,
tool-free summary request. A failed summary leaves the existing context or
checkpoint in use and is recorded as a compaction failure event.

`keep_recent_tokens` is the approximate size of the exact message suffix kept
beside the summary. It uses a provider-neutral estimate for choosing completed
message boundaries and keeps a tool call with its result. Diagnostic reasoning
text that provider adapters do not replay is excluded; replayable opaque
provider items remain included.
`summary_max_output_tokens` limits the summary request. Compaction requests are
additional provider calls and do not consume a normal agent `max_steps` slot.
Runs whose tool allowlist removes either history tool, or removes both `read`
and `bash`, keep their full context instead of compacting without an
exact-recovery path.

`history_search_max_matches` is a positive, per-query cap for newest-first
regex matches over messages removed from the active context. It is not an
artifact byte limit: matches omitted by this cap are not placed in the result
artifact. `history_search` and `history_read` have no cursor and never modify
the transcript; refine the regex or read a bounded window around a returned ref.
The local reader uses `rg` from `PATH` to scan linked full-text artifacts
without loading them into the Rust heap; message-only matching does not require
that subprocess. Remote readers may implement the same interface directly.

Compaction is local and model-generated. Picoagent does not currently call a
provider's server-side compaction API.

## Background Tasks And Agent Profiles

```toml
[tasks]
default_execution_timeout_seconds = 300
default_wait_timeout_seconds = 30
max_execution_timeout_seconds = 1800
direct_tool_timeout_seconds = 300

[agents.general_task]
# model = "smaller-compatible-model" # defaults to the primary provider model
max_steps = 8
max_output_tokens = 4096
```

Execution timeouts are hard bounds. A `wait` timeout only stops waiting; it does
not cancel the task. The runtime also enforces `max_parallel_tasks` across
background tools and child agents in one parent run. On Unix, `bash` commands
run in a dedicated process group so cancellation also terminates descendants.

## Artifacts

```toml
[artifacts]
inline_bytes = 32768
max_inline_bytes_per_run = 131072
preview_head_bytes = 8192
preview_tail_bytes = 8192
```

`max_inline_bytes_per_run` is a cumulative model-facing preview budget. Once it
is exhausted, even small results are stored as artifacts and only compact
references enter later model requests.

## Memory

```toml
[memory]
enabled = true
# global_root = "/persistent/pico-home"
```

`global_root` is the base containing `memory/user/`. Project memory always lives
at `<workspace>/.pico/memory/project/`. Memory is never written merely because a
run succeeded; the model must call `memory_update`, directly or through
`spawn`.

## Web Search

```toml
[web_search]
enabled = true
api_key_env = "BRAVE_SEARCH_API_KEY"
endpoint = "https://api.search.brave.com/res/v1/web/search"
default_count = 8
```

When enabled, the API key environment variable must exist at startup. Local
workspace search remains a `bash`/`rg` operation.

## MCP

Each `[mcp.<name>]` entry starts one stdio child process for the duration of the
job.

```toml
[mcp.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[mcp.github.env]
GITHUB_TOKEN = "${GITHUB_TOKEN}"
```

Values written as `$NAME` or `${NAME}` are resolved from the picoagent process
environment. Other values are passed literally.

## Hooks

Hook commands run synchronously in the configured order, receive JSON on stdin,
and may emit `{"payload": ...}` on stdout for the next hook.

```toml
[hooks]
run_start = ["./scripts/run-start.sh"]
run_end = ["./scripts/run-end.sh"]
tool_before = []
tool_after = []
```

Hooks inherit picoagent's host permissions. A nonzero hook exit fails the run.
