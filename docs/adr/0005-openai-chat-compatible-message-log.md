# ADR 0005: Persist Chat-Compatible Messages with a Metadata Sidecar

- Status: Accepted
- Date: 2026-07-16
- Refines: ADR 0001 (completed-message encoding)
- Refines: ADR 0003 (stable trajectory identity and recovery metadata)
- Refined by: ADR 0013 (sequence-addressed message refs)

## Context

The original `messages.jsonl` envelope mixed conversation content with
picoagent-only fields and tagged internal content variants. It was useful to the
Rust runtime, but a developer opening the trajectory did not see the familiar
OpenAI message shape. In particular, the synthetic runtime reminder appeared as
a `runtime_reminder` JSON type even though providers receive it as user text.

Persisting only a provider wire request would lose information needed by the
provider-neutral runner. Tool failures, stable message refs, timestamps,
internal block ordering, background task identity, and opaque continuation
items are not all representable in an OpenAI Chat message. A durable append also
needs an unambiguous crash boundary across the human-facing message and this
local state.

The project is still pre-release and no existing run requires migration, so a
legacy decoder would add code and mixed-format behavior without protecting a
real user workflow.

## Decision

- `run.json` declares the durable `message_format` as
  `openai-chat-compatible`. The name identifies the contract directly and does
  not carry a speculative `v1` suffix.
- Each `messages.jsonl` line is one complete Chat-compatible message:
  - user messages contain `role` and string `content`;
  - assistant messages contain `role`, string `content`, optional function
    `tool_calls`, and optional `reasoning_content`;
  - tool messages contain `role`, `tool_call_id`, and string `content`.
- Function arguments inside `tool_calls` are JSON-encoded strings, matching the
  Chat contract. If a compatible provider omits a streamed tool-call id, the
  adapter supplies a unique `call_<ULID>` id before the completed assistant
  message is persisted.
- The `<runtime-reminder>` block is ordinary text at the start of the first
  user message. No picoagent content type or local identity field is added to a
  message line.
- `reasoning_content` is an explicitly documented OpenAI-compatible endpoint
  extension, not an official OpenAI Chat Completions field. It is omitted when
  the provider does not return explicit reasoning text and is replayed only as
  the same separate field on compatible Chat requests.
- `message_metadata.jsonl` contains one paired local record for each message.
  It stores the stable message id, sequence, timestamp, SHA-256 of the exact
  message JSON, content-layout information, tool-error state, and any opaque
  provider continuation items needed to reconstruct the provider-neutral
  runtime message. A second SHA-256 covers the complete reconstruction payload,
  including refs, time, layout, tool-error state, and opaque items.
- The store writes and syncs the Chat line first, then writes and syncs the
  metadata line. Metadata is the commit marker. Readers expose only paired,
  hash-valid records. A lone final message is an interrupted append and is
  removed before the next append; metadata ahead of messages, an earlier count
  mismatch, a hash mismatch, or malformed committed data is corruption.
- Run creation pre-creates and directory-syncs both logs. Reads, recovery, and
  paired appends hold a per-run file lock; cached append positions are trusted
  only while both durable file lengths still match and are invalidated before
  cancellable I/O.
- Compaction and history retrieval operate on reconstructed committed messages.
  They do not change the Chat-shaped durable representation.
- The loader supports this format only. Runs created with the previous private
  envelope are intentionally not migrated or decoded.
- If a future change is backward-compatible, it can extend this named contract.
  A genuinely incompatible representation must use a different
  `message_format` name and an explicit migration decision.

## Consequences

- Developers can inspect or search `messages.jsonl` using familiar Chat fields,
  and the file contains no `runtime_reminder` variant or picoagent identity
  fields.
- `messages.jsonl` remains useful across OpenAI-compatible, Responses,
  Anthropic-compatible, and echo runs because it is the chosen durable
  projection, not a claim that every provider used the Chat wire endpoint.
- Runtime reconstruction requires the sidecar. Copying only `messages.jsonl`
  preserves a readable conversation but not stable refs, exact internal layout,
  tool-error classification, or opaque continuation state.
- A two-file commit protocol creates a possible final uncommitted Chat line,
  but it is detectable and safely ignored or repaired. Committed content is
  protected by the exact-message and reconstruction-metadata digests.
- Avoiding a legacy decoder keeps one storage contract during pre-release
  development, at the cost of making earlier development runs unreadable by
  the new runtime.

## Alternatives Considered

- **Keep the provider-neutral envelope in `messages.jsonl`.** Rejected because
  it makes routine trajectory inspection require knowledge of picoagent's
  internal enum and caused the reminder to look like a provider-visible custom
  message type.
- **Add picoagent fields directly to Chat messages.** Rejected because the
  resulting lines would no longer be clean Chat-compatible messages.
- **Use the OpenAI Responses item format.** Rejected as the durable projection
  because the configured launch workflow uses Chat Completions and the desired
  inspectable role/content/tool-call form is the Chat contract. Responses
  continuation items can remain opaque sidecar state.
- **Duplicate complete messages in the sidecar.** Rejected because it doubles
  durable content and creates two competing transcript representations. Byte
  ranges and indexed references retain only the reconstruction data that Chat
  cannot express.
- **Put `v1` in the format name.** Rejected because there is only one supported
  contract, compatible additions need no rename, and an incompatible future
  contract should receive a descriptive new identity rather than relying on a
  preallocated suffix.
- **Read both the old and new formats.** Rejected because no user depends on old
  runs and compatibility would add branching to every load and recovery path.

## Related Documents

- [ADR 0001: Durable messages and transient deltas](0001-durable-messages-transient-stream-deltas.md)
- [ADR 0003: Append-only compaction and history retrieval](0003-append-only-local-compaction-and-history-retrieval.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Design choices](../design-choices.md)
- [OpenAI Chat API reference](https://developers.openai.com/api/reference/resources/chat)
