# Design Choices

## Internal Harness First

Picoagent currently has no external users or compatibility commitments. It is
an internal harness for its maintainers' own workflows, so operator convenience,
code readability, and fast iteration take priority over broad configurability,
backward compatibility, and abstractions for hypothetical consumers.

Rejected: carrying compatibility layers, packaging indirection, or generalized
extension points without a concrete internal need. Revisit this boundary only
when an actual external consumer or distribution requirement exists.

## One Rust Runner

Main tasks and subagents use one `AgentRunner`. Child runs differ only by parent
id, depth, task instructions, and their own persisted run directory.

Rejected: a separate orchestrator agent type or child-specific model loop. That
would duplicate tool, provider, streaming, and persistence behavior.

## Two Model Timeout Scopes

Each provider call has a resettable stream-idle timeout and a non-resetting hard
deadline. Valid SSE events reset the idle interval even when they carry
reasoning, tool arguments, usage, or protocol state rather than visible text.
The hard deadline still bounds a connection that makes negligible progress
forever. Tool execution and model-slot queueing are outside both scopes.

Rejected: one hard timeout for the entire stream, because it kills a long but
healthy reasoning response; and an idle timeout alone, because heartbeat or
trickle traffic could retain a model slot indefinitely. See
[ADR 0011](adr/0011-model-stream-idle-timeout-and-request-deadline.md).

## Headless First

The runtime emits structured events and portable artifacts. It does not contain
a TUI or web frontend.

Rejected: preserving the legacy Bun/Hono/Ink surfaces. A future service can wrap
the Rust library and event stream without owning agent behavior.

## File-Based Runs

The launch runtime uses one self-contained directory per run instead of SQLite
or an event-sourced service. Complete messages and metadata are enough for
inspection and form the boundary for the bounded `pico resume` command; object
storage can archive the directory as a unit.

The persisted event log contains lifecycle and debugging records, not streaming
text or reasoning chunks. Complete messages are the searchable trajectory;
live event sinks carry transient deltas for interactive consumers.

Within a run, completed messages use the short ref `m<N>`, where `N` is the
durable one-based sequence. This gives history tools an explicit age/order
signal without exposing a separate sequence field or asking the model to parse
opaque ULIDs. Steering input ids remain separate recovery metadata.

See [ADR 0001](adr/0001-durable-messages-transient-stream-deltas.md) for the
decision context and rejected alternatives.

Revisit when cross-run queries, multi-worker ownership, or server-side pagination
become concrete requirements.

## Completed-Message Resume

Main runs and durable GeneralTask children resume from committed complete
messages. A run-level lease
prevents concurrent advancement. Missing direct-tool results become explicit
`interrupted` error results rather than replaying potentially side-effecting
work. Durable task records coordinate parent and child state, while each child
keeps its own transcript and result delivery is derived from the parent log.
The parent is the recovery entrypoint for every GeneralTask child, including
one delegated to a large memory update.

Rejected: replaying incomplete tools, copying child messages into task JSON,
and maintaining a second durable `delivered` boolean. See
[ADR 0006](adr/0006-complete-message-resume-and-durable-child-coordination.md).

## Chat-Compatible Message Log

`messages.jsonl` is directly inspectable as `openai-chat-compatible` messages,
without picoagent ids or content variants mixed into the JSON. The initial
runtime reminder is text in the first user message. Explicit compatible-endpoint
reasoning uses the optional `reasoning_content` extension; it is not an official
OpenAI Chat field.

Stable refs, sequence and time, exact-message and reconstruction-metadata
hashes, internal layout, tool-error state, and opaque provider continuation
items live in a paired `message_metadata.jsonl` sidecar. Writing metadata last
gives each pair a simple commit boundary while preserving enough information to
reconstruct the provider-neutral runtime message. Per-run file locking keeps
independent store instances from interleaving the two halves of a commit.

Rejected: a picoagent-specific envelope in the message log, private fields on
Chat messages, duplicating all message text in metadata, and preserving a
legacy decoder before any released run depends on it. The unversioned
`openai-chat-compatible` name describes the contract; an actually incompatible
future representation must use a distinct format name rather than reserving a
speculative `v1` suffix. See
[ADR 0005](adr/0005-openai-chat-compatible-message-log.md).

## Append-Only Local Compaction

Compaction reduces the active model request without changing the raw evidence:
complete messages remain append-only, and successful compaction instructions
and assistant states are recorded in the same Chat-compatible log. Exact
compacted details remain available
through read-only regex search and ref-centered reads; the retrieval interface
can be backed by local files or a future remote store.

Rejected: destructive transcript rewriting, cursored pagination in the initial
tool contract, vector retrieval without a demonstrated need, and relying on a
provider-specific server-side compaction API. See
[ADR 0012](adr/0012-record-compaction-as-messages.md).

## Uniform Background Delivery

Delegated children and promoted tools use one runtime notice shape. A
status-less task block means work is running; a terminal block contains only
the complete result artifact path. Ready terminal tasks share one runtime
message, and internal kind or provider call ids stay out of model-facing XML.

Rejected: separate start/result protocols, inline terminal previews, and one
runtime message per ready task. See
[ADR 0020](adr/0020-unify-background-task-runtime-notices.md).

## Artifact-First Tool Output

Large results are preserved in full but represented in model context by a small
versioned envelope. This was chosen over destructive truncation and over placing
unbounded stdout in every subsequent model request. Each result is limited
independently; picoagent does not retain a cumulative preview budget across a
run because compaction can free context and later small results should remain
directly readable. See [ADR 0018](adr/0018-limit-tool-output-per-result.md).

## Markdown Memory

Memory is human-editable Markdown outside the transcript. Ordinary `read`,
`write`, and `bash` capabilities inspect and update it. Small changes stay in
the current run; a large independent consolidation uses an ordinary durable
GeneralTask child.

Rejected for launch: a dedicated memory tool or profile, vector databases,
automatic recording of every successful run, Rust-side semantic heuristics,
and making raw transcripts or artifacts equivalent to curated memory. See
[ADR 0009](adr/0009-memory-through-ordinary-tools.md).

## One Background Task Lifecycle

Direct calls from one assistant message start concurrently under one shared
foreground window. Results remain in original call order; only unfinished exact
futures move to the background. `delegate` starts a GeneralTask child
asynchronously. Separate `task_status`, `task_wait`, `task_inspect`,
`task_steer`, and `task_stop` tools keep each schema small and explicit.
Agent loops have no arbitrary model-step cap, and background work has no hard
execution deadline. See [ADR 0017](adr/0017-concurrent-tool-batches-and-explicit-task-controls.md).

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

Image reads use one canonical attachment block. Each provider adapter owns its
native multimodal projection, while the Chat-compatible log stores native user
content parts and the sidecar commits reconstruction layout. Direct batches
emit all paired tool results before one attachment message, avoiding ambiguous
interleaving under concurrent completion. See
[ADR 0022](adr/0022-native-image-attachments-after-tool-results.md).

## Stable Prompt Prefix

The built-in system prompt contains only product identity and stable operating
rules. Workspace `AGENTS.md`, skill metadata, memory paths, and delegated-task
instructions are snapshotted into a synthetic runtime reminder at the start of
each run. Tool
descriptions remain in sorted tool schemas rather than being duplicated in the
system prompt. Core history schemas are present from the first normal call.
Root and GeneralTask use the same built-in schema set and freeze it for the run;
compaction reuses the same system and schemas. Remaining delegation depth is
persisted runtime state and never changes schema membership. Optional startup
capabilities are resolved before the run starts. Memory paths do not alter the
tool schema. See
[ADR 0024](adr/0024-freeze-built-in-schemas-across-agent-roles.md).

The system prompt contains one stable rule for model modalities, while the
runtime reminder snapshots the configured values. The provider config defaults
to text-only and does not guess from model names. `read` rejects image input at
its execution boundary when `image` is absent. Rejected: endpoint probing,
model-name allowlists, and per-agent dynamic vision routing. See
[ADR 0023](adr/0023-declare-model-input-modalities.md).

Rejected for launch: conditionally adding history tools and hot-reloading
project context or tool definitions inside a run. Appending revisions would
grow context, while replacing earlier messages would break the durable
transcript boundary and provider prefix-cache
reuse. See [ADR 0004](adr/0004-stable-agent-prefix-and-core-history-tools.md).

## Compile-Time Prompt Assets

Stable agent instructions are folded values in one typed `agents.yaml` registry;
each local tool keeps its static name, folded purpose description, folded return
guidance, and input schema in one typed `tool.yaml` beside its adapter. The
loader joins the two prose fields into the provider's standard description.
These assets are embedded with `include_str!` and parsed strictly. Rust remains
authoritative for prompt assembly, argument validation, and execution.
Every local model-facing adapter keeps its complete manifest beside its Rust
module. Standalone tools stay directly under `src/tools`; cohesive task and
history adapters are grouped by family without deriving model-visible names
from paths. Domain engines remain in their focused subsystems. Process and run
capabilities are assembled through one explicit path; ordinary tools are called
directly, while `delegate` and the task controls are complete static adapters.

See [ADR 0019](adr/0019-group-related-tool-adapters.md),
[ADR 0016](adr/0016-separate-tool-purpose-and-return-guidance.md),
[ADR 0015](adr/0015-local-tool-yaml-manifests.md),
[ADR 0014](adr/0014-flat-tool-adapters-and-explicit-assembly.md), and
[ADR 0008](adr/0008-typed-agent-prompt-registry.md) for the packaging and
ownership decisions.

## External Scheduling

Memory consolidation is a command. Cron, systemd, Kubernetes, or another job
platform decides when it runs.

Rejected: an embedded scheduler and daemon lifecycle in the launch harness.
