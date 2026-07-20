# AGENTS.md

This file is the operating map for agents working in this repository. Keep the
product and user workflow in `README.md`, current tradeoff summaries in `docs/`,
individual architectural decisions in `docs/adr/`, and this file focused on
navigation, invariants, verification, and handoff.

## Source Map

- `src/agent/`: the single agent loop, run state, prompt assembly, local
  compaction, and child-run supervision. Main runs and subagents must use the
  same runner.
- `src/model/`: provider-neutral message/tool contracts plus OpenAI OAuth,
  OpenAI-compatible, Anthropic-compatible, and deterministic echo adapters.
- `src/tools/`: the stable tool contract and registry plus flat standalone base
  tools (`read`, `write`, `bash`, and optional `web_search`). Each base tool owns
  its implementation, schema, and compile-time Markdown description.
- `prompts/agents.yaml`: the typed registry of stable agent-level instructions
  embedded into the binary; dynamic assembly remains in `src/agent/context.rs`.
- `src/artifact.rs`: large-output spill, previews, immutable artifact metadata,
  and project-local artifact paths.
- `src/storage/`: self-contained run directories, Chat-compatible message JSONL
  with paired local metadata (including compacted-state boundaries), event
  JSONL, status, and final-result persistence.
- `src/trajectory.rs` and `src/trajectory/`: provider-neutral compacted-history
  search/read contracts plus the local message and artifact reader.
- `src/skills/`: Agent Skills discovery and progressive `SKILL.md` loading.
- `src/mcp.rs`: MCP stdio connection lifecycle and tool adapters.
- `src/hooks.rs`: command hook discovery and lifecycle invocation.
- `src/memory.rs`: user/project Markdown paths exposed to ordinary agent tools.
- `src/config.rs`: `.pico/config.toml` loading and runtime/provider settings.
- `src/events.rs`: transport-neutral runtime events and event sinks.
- `src/cli.rs`: command-line shape; `src/main.rs`: headless composition root.
  Runtime behavior does not belong in either file.
- `tests/`: cross-module and end-to-end behavior.
- `docs/`: architecture, artifact, memory, configuration, and runtime contracts.
- `docs/adr/`: numbered Architecture Decision Records explaining durable
  decisions, alternatives, and consequences.

## Engineering Invariants

- Rust is the only implementation language for the harness.
- Keep one `AgentRunner`; a subagent is a child run with a parent id and a
  constrained depth, not a second loop or agent class.
- Keep provider wire formats and auth outside the agent loop.
- Keep one deterministic, namespaced tool registry. MCP tools adapt into the
  same `Tool` contract and cannot silently replace built-ins.
- Treat completed messages as the resumable boundary. Stream deltas are events,
  not durable conversation messages.
- Keep `messages.jsonl` in the declared `openai-chat-compatible` shape. Store
  ids, sequence, timestamps, exact-message and reconstruction-metadata hashes,
  tool-error state, and opaque provider items in the paired
  `message_metadata.jsonl`; metadata commits the already-synced message line.
- Serialize message-log reads, recovery, and paired appends with the per-run
  file lock. In-memory cursors are only a fast path and must be invalidated
  before cancellable writes or whenever durable file lengths change.
- Spill large tool results to `.pico/runs/<run-id>/artifacts/`; preserve the full
  result and return a bounded head/tail preview plus an immutable artifact ref.
- Enforce both the per-result inline threshold and the cumulative per-run preview
  budget; once the latter is exhausted, return artifact references without previews.
- Keep artifact ids and metadata stable. Changing content under the same hash or
  identity is a contract violation.
- Keep prompt prefixes deterministic: stable section order, sorted tools and
  skills, and dynamic memory/tool results near the tail.
- Keep the normal agent system prompt invariant. A compaction request uses that
  same prompt and frozen tool schemas, plus one final user instruction; reject
  tool calls instead of executing them during compaction.
- Register `history_search` and `history_read` before the first normal provider
  call regardless of `compaction.compact_at_tokens`. That setting controls
  compacted-state creation only; sorted tool schemas stay frozen per run.
- Keep Root and delegating/leaf GeneralTask as explicit capability profiles,
  and use the run's existing profile for compaction. Select the GeneralTask
  variant from remaining depth before its run starts; do not add or remove
  schemas during a run.
- Keep stable agent prose in the typed compile-time YAML registry and tool
  descriptions beside their owners. Keep prompt assembly, schemas, validation,
  and execution contracts in Rust.
- Memory is durable user/project knowledge outside the live transcript. Inspect
  and update its ordinary Markdown with the general tools; do not inject the
  tree into every prompt or add a memory-specific tool.
- Keep user memory and project memory distinct. Raw artifacts and transcripts
  are sources, not automatically curated memory.
- The launch runtime intentionally has no security sandbox or approval engine.
  Tools and hooks inherit the picoagent process permissions; document this
  plainly and do not imply otherwise.
- Treat picoagent as an internal harness with no external compatibility promise.
  Optimize for maintainer convenience, readability, and fast iteration; do not
  add compatibility layers or generality for hypothetical users.
- Do not add a TUI, frontend framework, built-in scheduler, vector database,
  native dynamic plugin ABI, or distributed worker system without a concrete
  request.
- Prefer a readable module over speculative framework layers or defensive code
  for states the program cannot produce.

## Artifact Contract

- Small UTF-8 tool results may stay inline.
- Large results must preserve their complete bytes under the current run.
- A truncated model-facing result must include `truncated`, total bytes, media
  type, hash, stable relative path, and useful beginning/end previews.
- `read` must support bounded reads so the model can inspect an artifact
  without loading it all back into context.
- Run directories and artifact manifests are portable job outputs. Avoid
  machine-specific absolute paths in persisted references.

See `docs/artifacts.md` for the complete format.

## Verification

Run for every code change:

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

For provider changes, run the mock-server contract tests covering streamed text,
fragmented tool arguments, error responses, and authentication refresh behavior.

For runtime or artifact changes, also run a headless smoke task with the echo
provider and inspect the generated run directory, `messages.jsonl`,
`message_metadata.jsonl`, `events.jsonl`, final output, and artifact metadata.

For prompt or tool-description asset changes, verify `cargo package --list`
contains every referenced asset in addition to compiling all targets.

## Docs Update Rules

- User-visible commands, setup, or supported features: update `README.md`.
- Runtime or module boundaries: update `docs/architecture.md`.
- Artifact envelope, spill threshold, paths, or cleanup: update
  `docs/artifacts.md`.
- Memory scopes, update behavior, paths, or consolidation: update `docs/memory.md`.
- Config fields or provider behavior: update `docs/configuration.md`.
- Prompt asset organization or assembly behavior: update `prompts/README.md`
  and `docs/architecture.md`.
- Current high-level tradeoff summaries: update `docs/design-choices.md`.
- Significant cross-module decisions, durable invariants, or rejected credible
  alternatives: add or supersede an ADR under `docs/adr/` and update its index.
  Do not silently rewrite an accepted decision when the architecture changes.

## Review And Handoff

- Review module size and ownership before adding to a file already near 400
  lines; split by behavior, not by arbitrary helper categories.
- Distinguish correctness checks from speculative defense. Keep checks at real
  external boundaries and remove unreachable fallback layers.
- Report tests actually run and any provider path not exercised with live
  credentials.
- Do not revert unrelated user changes.

## Commit Rules

- Use Conventional Commits with a body explaining what changed and why.
- Keep generated run directories, credentials, and target output out of Git.
