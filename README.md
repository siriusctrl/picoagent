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
- compact `read`, `write`, and `bash` built-ins plus optional `web_search`;
  `read` supports bounded UTF-8 text and image attachments for vision-capable
  configured models
- exact-first, atomic multi-edit writes with CRLF/BOM preservation
- versioned artifact spill for large tool results with bounded head/tail previews
- optional local context compaction recorded as ordinary messages, with
  read-only regex history retrieval
- Agent Skills discovery with progressive `SKILL.md` loading
- MCP stdio servers adapted into the same tool registry
- command hooks for run and tool lifecycle events
- concurrent direct-tool batches whose unfinished calls continue through
  generic background task control
- asynchronously delegated general-task subagents that reuse the same runner
- run-local YAML planning graphs maintained with ordinary file tools
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
  pending_inputs.jsonl # created when a running child is steered
  events.jsonl
  final.md
  artifacts/
  graphs/
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
own transcripts. Durable GeneralTask children created by `delegate` continue
when their parent is resumed; resume the parent run rather than invoking `pico
resume` on a child id directly.

## Prompt Layout

Picoagent keeps its built-in system prompt independent of the workspace and
unchanged across normal agent calls. At the start of each run, the first user
message contains a synthetic `<runtime-reminder>` block before the original
request. The reminder snapshots the workspace path, `AGENTS.md`, discovered
skill metadata, memory locations, and any delegated-task instructions that
apply to the role. It also records the role and remaining delegation depth;
GeneralTask guidance lives here rather than in a second system prompt. Built-in
tool schemas are identical for Root and GeneralTask, sorted, and frozen for the
run; configuration or file changes take effect on the next run. The environment
section also states `current model supported
modalities: [text]` (or `[text, image]`); the stable system prompt tells the
agent not to request an absent modality. Compaction reuses this stable
system/tool prefix and adds one final user instruction only to the compaction
request.

`messages.jsonl` uses the self-contained `pico-message` format. Each line has a
short `ref` (`m1`, `m2`, ...), `created_at`, `role`, and typed `content` blocks.
Those blocks are the exact provider-neutral messages replayed by the runner, so
tool failures, artifact refs, images, reasoning, and opaque provider
continuation items need no sidecar or reconstruction layout. Optional steering
idempotency and compaction classification live under `_pico` on the same line.
The sequence is derived from `ref` and the line position rather than duplicated.

A newline commits the record. The single process holding the run execution
lease is the only writer; any number of viewers may read complete lines without
taking a message-log lock. A viewer ignores an incomplete final line, and the
writer trims that tail before resuming appends. `run.json` declares
`"message_format": "pico-message"`, retains the original user prompt, and
freezes the stored profile plus remaining delegation depth. Compaction never
rewrites or deletes committed trajectory records.

Stable agent instructions are folded scalar values in the typed, compile-time
`prompts/agents.yaml` registry. Every local model-facing tool adapter has a
typed `tool.yaml` beside it; standalone tools live under `src/tools/<tool>/`,
while task, history, and graph families use
`src/tools/<family>/<member>/`. The manifest
always owns the complete model-facing name, purpose description, return
guidance, and input schema. Rust composes the two prose fields into the standard
provider description and owns validation, assembly, and execution.

## File-backed Planning Graphs

For a complex task, `graph_init` accepts the goal and complete initial node map,
validates the DAG, and creates a short run-local path such as
`.pico/runs/<run-id>/graphs/g1.yaml`. Invalid initialization creates no file.
The graph is durable coordination state, not a scheduler: nodes are work items,
dependencies are accepted-outcome dependencies, and a node is resolved only
when the main agent writes a resolution. Use ordinary `read` and `write` for
later revisions, then call `graph_list` to validate them and derive ready nodes.
Execute independent ready work with concurrent `delegate` calls and supervise
those runs with the existing task controls; task ids are not stored in the
graph.

```json
{
  "goal": "Implement and verify image input support",
  "nodes": {
    "inspect_api": {
      "objective": "Determine the provider request contract",
      "depends_on": []
    },
    "implement": {
      "objective": "Implement the accepted contract",
      "depends_on": ["inspect_api"]
    }
  }
}
```

Tool calls within one assistant response run concurrently. Therefore later
graph updates are a three-turn dependency chain: complete `write`, call
`graph_list` only after receiving that result, and issue dependent `delegate`
calls only after receiving the validated listing. Do not batch dependent stages
together.

```yaml
version: 1
status: wip
goal: >-
  Implement and verify image input support.
nodes:
  inspect_api:
    objective: >-
      Determine the provider request contract.
    depends_on: []
    resolution:
      summary: >-
        The request contract is documented.
      evidence:
        - .pico/runs/<run-id>/artifacts/api-contract.txt
  implement:
    objective: >-
      Implement the accepted contract.
    depends_on: [inspect_api]
    resolution: null
```

`graph_list` groups valid files as `wip`, `completed`, or `aborted`, reports
resolved/unresolved counts, and derives ready node ids. Malformed YAML, unknown
or repeated dependencies, dependency cycles, unsafe evidence paths, and
inconsistent terminal state appear under `invalid` without hiding other valid
graphs. A completed graph requires every node to be resolved and a non-empty
top-level `summary`; an aborted graph requires a non-empty `abort_reason` and
always reports `ready: []`. A resolution is invalid while any of that node's
direct dependencies remains unresolved.

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
modalities = ["text"]
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
modalities = ["text"] # use ["text", "image"] only for a vision model
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
modalities = ["text"]
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
```

See [configuration.md](docs/configuration.md) for runtime, compaction, artifact,
MCP, hook, and memory settings.

## Context Compaction And History

Automatic local compacted-state creation is disabled by default. Enable it with
thresholds appropriate to the model's context window:

```toml
[runtime]
max_output_tokens = 8192

[compaction]
compact_at_tokens = 100000
context_window_tokens = 131072
keep_recent_tokens = 20000
summary_max_output_tokens = 4096
history_search_max_matches = 50
```

Picoagent estimates the complete input from the first request and replaces that
estimate with provider-reported input usage whenever available. It can make an
additional model call when the tracked input reaches `compact_at_tokens`. The
call receives the original initial message, any previous compacted state, the
native older messages being replaced, and one final compaction instruction.
The successful user instruction and exact assistant response are appended to
`messages.jsonl`; the assistant response is the durable compacted state. Later
normal requests omit the compaction instruction and contain that exact
assistant message plus the exact recent suffix.

`context_window_tokens` is the model's configured nominal full window. Before
normal and compaction requests, picoagent checks its provider-neutral estimate
of system, schemas, active messages, and configured output allowance. It must
be greater than `compact_at_tokens`; if compaction cannot reduce the estimate
below it, the run fails locally. This is an early safety check, not a
tokenizer-exact provider guarantee. Setting the window requires an explicit
nonzero `runtime.max_output_tokens`; GeneralTask uses its separately configured
profile limit. Start/completion/failure records remain in `events.jsonl`;
compaction retries have numbered attempts, while a preflight rejection has no
started event or attempt because no provider request occurred. Compatible Chat `reasoning_content` and replayable opaque
provider items are included in the between-call estimate.

The normal agent receives the `history_search` and `history_read` schemas from
its first provider request whether or not `compact_at_tokens` is configured.
Changing the threshold controls compacted-state creation only; it does not
change the normal system prompt or tool schemas. Before anything has been
compacted, the history tools simply have no compacted prefix to search or read.

Two read-only tools recover exact details omitted from the active request:

- `history_search({"pattern":"..."})` applies a Rust regular expression only
  to compacted messages and their linked textual tool-result artifacts. Results
  are newest-first. Each match contains `ref`, `source`, and `snippet`: refs are
  run-local sequence addresses such as `m37` (smaller numbers are older), while
  `source` is `message` for inline content or `artifact` for a linked complete
  spilled result.
- `history_read({"ref":"m37","before":2,"after":2})` reads a bounded
  chronological window around one ref. It returns JSONL records shaped as
  `{"ref":"m<N>","message":<OpenAI Chat-compatible message>}` and keeps tool
  calls paired with their results.

The local reader invokes `rg` for bounded-memory searches inside full textual
artifacts, so ripgrep must be available on `PATH` for that part of
`history_search`. A future remote reader can provide the same contract from a
database or service.

Neither tool has a cursor. `history_search_max_matches` limits a query to its
newest matches; if reached, older matches are omitted and the model must refine
the regex. This is distinct from artifact preview truncation: if the bounded
JSON/JSONL tool result is too large, its complete returned content is saved as
an artifact and can be inspected with `read`, continuing from the returned
`line_offset` or `byte_offset`; `bash`/`rg` is also useful for targeted searches. Query-limit
omissions are not present in that artifact.

Each assembled agent run has a sorted, frozen toolset. A run compacts
only when both history tools and at least one artifact inspection tool (`read`
or `bash`) are present. The compaction request reuses the same system prompt and
tool schemas; a tool-call response is rejected rather than executed.

This release implements local model-generated compacted states only. It does not use
OpenAI or another provider's server-side compaction API.

## Large Tool Results

Small results are returned inline. Large results are written in full under the
current run and replaced in model context with:

- beginning and ending previews
- byte length and media type
- SHA-256 digest
- stable project-relative path

The model can inspect the complete output with bounded `read` calls or search it
with `bash` plus `rg`. Every tool result is limited independently; a previous
large result never suppresses a later small result. See
[artifacts.md](docs/artifacts.md).

## Tools And Background Work

The launch tool surface is intentionally small:

- `read`: bounded UTF-8 reads for a known path, or model attachments for jpg,
  jpeg, png, gif, webp, and bmp images
- `write`: full-file creation/replacement or an atomic list of targeted edits
- `bash`: local discovery, `rg`, tests, builds, and other Bash commands; returns
  combined stdout/stderr and adds a status line only for unsuccessful
  completion. It uses a non-login shell and inherits picoagent's environment
  without loading profile files
- `history_search`: regex search over the compacted trajectory prefix
- `history_read`: a bounded message window around a returned history ref
- `load_skill`: progressive loading of a catalogued skill's full instructions
- `delegate`: asynchronously start a GeneralTask child
- `task_status`: inspect current task state
- `task_wait`: wait for selected tasks for one bounded interval
- `task_inspect`: read a bounded window of a child agent's messages
- `task_steer`: queue input for a running child agent
- `task_stop`: cancel one background task
- `web_search`: optional Brave-backed public web search

Root and GeneralTask receive the same built-in schemas, including `delegate`
and every task control. Remaining delegation depth is frozen in run state and
shown in the runtime reminder. At zero, `delegate` returns a local tool error
without creating a task; its schema does not disappear. Memory adds paths to
the reminder, not a tool schema. `web_search` and MCP tools depend on startup
configuration. The resulting schemas are sorted and frozen before the run's
first normal provider call.

`write` requires every edit target to identify one non-overlapping region in
the original file. It tries exact matching first, then a conservative whole-line
indentation normalization. It does not use broad fuzzy similarity that could
silently modify the wrong code.

All direct tool calls in one assistant message start concurrently and share one
foreground window. If all finish early, picoagent returns immediately. At the
configured deadline, it preserves each unfinished exact future, moves only
those calls into the background task lifecycle, and returns their task ids; no
tool is stopped or restarted. Tool-result messages are committed in the
assistant's original call order and retain their original `tool_call_id`, even
though completion events can arrive in another order. The model should put only
independent calls in one batch and issue dependent work after seeing results.
The tool result is a status-less `<background_task>` notice containing the task
id and name; it only acknowledges that work is running.

`delegate` requires a short model-supplied name and starts a `general-task`
child asynchronously. The five `task_*` tools observe and control delegated
children and automatically promoted direct tools. Terminal background results
are always preserved as artifacts. At the next model boundary, one user/runtime
message batches every ready `<background_task status="...">` notice; each body
is only the complete artifact path. The model must read that path before using
the result. Internal task records retain promoted calls' original provider ids,
so the provider sees exactly one result for each original tool call.

The task-control calls are intentionally small:

```text
delegate({"name":"inspect_tests","prompt":"inspect the failing tests and report the cause","context":"fork"})
task_status({"task_ids":[]})
task_wait({"task_ids":["t1"]})
task_inspect({"task_id":"t1","limit":6,"before_seq":42})
task_steer({"task_id":"t1","message":"check the failing test first"})
task_stop({"task_id":"t1"})
```

An empty `task_ids` means all tasks owned by that run. `before_seq` is exclusive
and optional; inspect returns
`next_before_seq` when older messages exist. Task ids are short references
local to their parent run (`t1`, `t2`, ...); child run ids remain internal
durable-storage identities.

## Skills

Picoagent discovers Agent Skills from lowest to highest precedence:

1. `$HOME/.agents/skills/*/SKILL.md`
2. `<workspace>/.agents/skills/*/SKILL.md`
3. `<workspace>/skills/*/SKILL.md`

Only skill name and description enter the stable prompt prefix. `load_skill`
returns the instruction body without repeating that metadata, plus the absolute
skill-directory path needed to resolve referenced files.

```bash
pico skills list
```

## Subagents

`delegate` asynchronously starts the sole model-facing `general-task` role;
there is no model-facing profile choice. The runtime reminder states the exact
remaining delegation depth. With the default `max_subagent_depth = 1`, the
first child has
zero remaining depth; `delegate` stays visible there but fails locally. A child
is another invocation of the same runner, not a second agent class. Each child:

- invokes the same `AgentRunner` and provider
- has a separate run id, transcript, events, and artifacts
- records its parent run id
- shares the working project, so it can inspect and modify the same files
- uses the configured GeneralTask output profile; fresh uses its configured
  model while fork inherits the parent's selected model
- cannot delegate another child at the default depth limit

Parent and child model requests share `runtime.max_parallel_model_calls`, which
defaults to one for compatibility with rate-limited endpoints. Delegated-child
capacity remains independently controlled by `runtime.max_parallel_subagents`.

Every `delegate` call chooses a context mode. `context = "fresh"` starts with
only the child runtime reminder and delegated prompt. `context = "fork"`
inherits the exact parent model input before the assistant message containing
the delegate call, then appends the child reminder and prompt. Fork siblings in
one assistant tool-call batch share the same boundary. The copied trajectory,
including compacted-state metadata, is stored in each child run so resume and
history retrieval do not depend on a live parent process.

`runtime.model_stream_idle_timeout_seconds` (default 300) stops a model stream
that produces no valid SSE event for that interval, while
`runtime.model_request_deadline_seconds` (default 3600) caps the complete model
API call even when the stream keeps making progress. Neither limit includes
tool execution or time spent waiting for the shared model slot.

Only child results return to the parent context; full child transcripts remain
in their own run directories. The parent stores only coordination state under
`tasks/`. On parent resume, terminal-result delivery is derived from the parent
transcript, while queued/running child runs continue from their own last
complete messages. This recovery guarantee applies to every durable GeneralTask
task record, including one used for a large memory update.

If the process stops after `delegate` creates that record but before its tool
result commits, resume reconstructs the original status-less acknowledgement
and continues the same child. It does not replay the delegation or expose the
provider call id in task notices.

The parent can inspect a child's latest messages (six by default), page
backward by sequence, and queue steering while it runs. Steering is stored as
an ordinary user message after the child's current assistant response and full
tool-call batch, immediately before its next model request. It does not
interrupt or discard in-flight tools.

## Long-Term Memory

Memory is durable knowledge about the user and projects, not the current
conversation. It is ordinary Markdown at two locations which are included in
an ordinary agent's initial runtime reminder:

- `$PICO_HOME/memory/user/` for cross-project user knowledge
- `<workspace>/.pico/memory/project/` for project-specific knowledge

There are no special memory tools. The model uses `read`, `write`, and `bash`
for small focused changes. For a large independent update, it can delegate an
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
        -> local Tool adapters grouped where related
        -> MCP Tool adapters
        -> TaskManager
           -> promoted direct Tool future
           -> delegated child AgentRunner
     -> ArtifactStore
     -> RunDirStore
     -> EventSink
```

Provider wire formats never enter the loop. MCP adapters use the same `Tool`
contract as local adapters. Subagents use the same runner. Large results use the
same artifact contract regardless of source.

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
