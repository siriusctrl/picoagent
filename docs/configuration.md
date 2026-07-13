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
api_key_env = "OPENAI_API_KEY"
protocol = "responses" # or "chat-completions"
```

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
