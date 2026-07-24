# ADR 0043: Compact Message and Checkpoint Payloads

- Status: Accepted
- Date: 2026-07-24
- Refines: ADR 0032 (provider-neutral message shape)
- Refines: ADR 0034 (checkpoint metadata encoding)
- Refines: ADR 0042 (opaque continuation items)

ADR 0044 later removes the remaining multi-message checkpoint metadata and
advances the run record to version 14. The compact message-content decisions
here remain accepted.

## Context

The self-contained message format retained three pieces of duplication. Tool
calls had identical fields in both `MessageContent` and `ToolCall`. Opaque
Responses items stored a provider label even though the run freezes its
provider and wire protocol. Every checkpoint member stored its first message
ref even though the member ref and zero-based index already determine it, and
singleton records repeated an index and count that their newline already
expressed.

The reverse transcript reader still needs both position and group size on any
multi-message tail line. Removing all checkpoint metadata or storing it only on
the first line would make bounded reverse reads scan an unknown prefix.

## Decision

- `MessageContent::ToolCall` directly contains the existing `ToolCall` value.
  Its internally tagged durable JSON remains `type`, `id`, `name`, and the
  exact argument string.
- `ProviderItem` stores only its opaque item. The run's frozen provider and
  protocol determine which adapter can replay it.
- `Message::new` supplies the ordinary absent `reasoning_content`, keeping
  role/content construction concise without introducing a role-specific
  message hierarchy.
- A singleton record omits `_fiasco.checkpoint`; its terminating newline is its
  commit marker.
- Every member of a multi-message checkpoint stores only zero-based `index` and
  shared `count`. Contiguous canonical `m<N>` refs provide group identity.
- The pre-release run record advances to version 13. Earlier development runs
  are not decoded or migrated.

## Consequences

- One tool-call shape is used by stream assembly, execution, provider
  projection, tests, and durable storage.
- Message lines no longer repeat a provider label, a derivable first ref, or
  singleton checkpoint metadata.
- Multi-message tool and compaction turns retain their existing atomic
  visibility and bounded reverse-read behavior.
- Existing version-12 development runs are rejected at the run boundary
  instead of mixing two message formats under one version.

## Alternatives Considered

- **Use a role-specific `Message` enum.** Rejected because it replaces a small
  boundary validation with pervasive matching and accessors.
- **Merge runtime reminders, handles, and tool results into text.** Rejected
  because retrieval, artifact, and provider-role semantics would need new
  metadata to distinguish them again.
- **Remove multi-message checkpoints.** Rejected because a crash could expose
  an assistant tool call without all required tool results.
- **Store checkpoint metadata only on the first member.** Rejected because a
  reverse reader starting at an incomplete tail could not locate the group
  boundary without an unbounded scan.
- **Keep the provider label for possible future adapters.** Rejected because no
  current run can switch provider or protocol, and hypothetical generality does
  not justify a redundant durable field.

## Related Documents

- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Design choices](../design-choices.md)
- [ADR 0032: Self-contained message log](0032-self-contained-message-log.md)
- [ADR 0034: Atomic turn checkpoints](0034-atomic-turn-checkpoints.md)
- [ADR 0042: Chat reasoning sibling field](0042-chat-reasoning-sibling-field.md)
