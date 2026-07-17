# Configuration

Picoagent reads TOML from an explicit `--config` path, the workspace
`.pico/config.toml`, or the user `$HOME/.pico/config.toml`. Workspace and user
files are alternatives in the launch runtime; they are not merged.
Unknown fields are rejected so misspelled settings fail at startup instead of
silently falling back to defaults.

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
shared. If `api_key` is omitted, picoagent reads `OPENAI_API_KEY`. The removed
OpenAI-compatible `api_key_env` field is rejected; use
`api_key = "${OPENAI_API_KEY}"` for an explicit environment reference.

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
from `delta.content`, persisted as the optional `reasoning_content` field on the
complete assistant line in `messages.jsonl`, and replayed in that separate field
on later compatible Chat requests. It is never concatenated into visible
assistant `content`. `reasoning_content` is an OpenAI-compatible endpoint
extension, not an official OpenAI Chat Completions message field. Empty deltas
are ignored. If usage includes
`completion_tokens_details.reasoning_tokens`, that count is written to the
`model_completed` event. Responses usage reports the equivalent count under
`output_tokens_details.reasoning_tokens`.

Some compatible endpoints omit the required id from a streamed tool call.
Picoagent assigns a unique `call_<ULID>` id at the provider boundary so the
assistant call and its tool result retain an unambiguous Chat identity.

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
max_subagent_depth = 1
max_parallel_tasks = 4
max_parallel_model_calls = 1
model_request_timeout_seconds = 300
max_output_tokens = 8192
```

Normal and child agent loops have no model-step cap. They continue until the
model returns a final response with no unresolved background work, the run is
stopped, or a real provider/runtime error occurs.
`max_parallel_model_calls` is shared by a parent and all of its child runs; the
conservative default of one supports endpoints with a single-request
concurrency limit, while higher-capacity deployments may raise it.
`model_request_timeout_seconds` bounds each normal or compaction request;
an expired normal request fails the run, while an expired compaction request
leaves the current context unchanged. Both parallel capacities, configured
output token limits, model request timeouts, and task wait/foreground limits
must be greater than zero.

The OpenAI-compatible adapter additionally retries initial HTTP 429 responses
up to three times with bounded exponential backoff. It does not retry a partial
stream or non-rate-limit provider error. A resumable run records a non-secret
fingerprint of wire-critical provider settings. Changing the compatible
endpoint, Chat/Responses protocol, reasoning effort, OAuth inference endpoint,
or Anthropic version requires a new run rather than replaying provider state
against a different protocol.

## Compaction And History Retrieval

```toml
[compaction]
# trigger_tokens = 100000       # omitted by default: automatic compaction off
keep_recent_tokens = 20000
summary_max_output_tokens = 4096
history_search_max_matches = 50
```

`trigger_tokens` enables automatic checkpoint creation and must be greater than
zero. It does not enable tools or change the normal system prompt: every normal
agent profile receives `history_search` and `history_read` from its first
provider call even when the setting is omitted. The summary and history-search
limits must also be positive. The trigger depends on the active provider
reporting input-token usage; a provider that omits it cannot trigger automatic
compaction. When the tracked context reaches the threshold, picoagent uses the
same provider and model for an additional, tool-free summary request. A failed
summary leaves the existing context or checkpoint in use and is recorded as a
compaction failure event.

`keep_recent_tokens` is the approximate size of the exact message suffix kept
beside the summary. It uses a provider-neutral estimate for choosing completed
message boundaries and keeps a tool call with its result. Compatible Chat
`reasoning_content` and replayable opaque provider items are included.
`summary_max_output_tokens` limits the summary request. Compaction requests are
additional tool-free provider calls.
A fixed profile without both history tools and at least one of `read` or `bash`
would keep its full context instead of compacting without an exact-recovery
path.

Root and a delegating or leaf GeneralTask each assemble a sorted tool registry
and freeze it before their first normal provider call. A GeneralTask's variant
is selected from the remaining delegation depth before its run starts. The
compaction summary profile deliberately has no tools. Delegation depends on the
selected depth variant; optional `web_search` and MCP schemas depend on startup
configuration. Memory uses the ordinary file tools and adds no schema. None
changes during the run.

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
foreground_tool_timeout_seconds = 300
wait_timeout_seconds = 30

[agents.general_task]
# model = "smaller-compatible-model" # defaults to the primary provider model
max_output_tokens = 4096
```

`foreground_tool_timeout_seconds` is a promotion window, not an execution
deadline. When it expires, the already-running direct tool continues as a
background task and the model receives its task id. Explicitly spawned tools
and subagents have no harness execution deadline. Each `task wait` call returns
after at most `wait_timeout_seconds` without cancelling work; this value must be
strictly lower than the foreground window. `task stop` performs cancellation.
The runtime enforces `max_parallel_tasks` across explicitly spawned tools and
child agents in one parent run. On Unix, cancelling `bash` terminates its process
group descendants too.

Failed background tool and child results use the same artifact threshold and
preview budget as successful results, so a large error is preserved without
being injected into the parent context in full.

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

`global_root` must be an absolute path and is the base containing
`memory/user/`. A relative `PICO_HOME` environment value is resolved against the
launch working directory. Project memory always lives at
`<workspace>/.pico/memory/project/`. Memory is never written merely because a
run succeeded. When enabled, the resolved paths enter the initial reminder; the
model uses `read`, `write`, and `bash` directly, or delegates a large independent
update through an ordinary GeneralTask child.

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

Hooks inherit picoagent's host permissions. A nonzero `run_start`,
`tool_before`, or `tool_after` exit fails that operation. `run_end` is a
best-effort post-commit notification: its failure is logged but cannot turn a
completed run back into a resumable failed run and replay earlier hook effects.
