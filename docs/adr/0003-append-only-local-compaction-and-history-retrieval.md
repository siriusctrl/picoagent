# ADR 0003: Add Local Compaction Without Rewriting the Trajectory

- Status: Superseded
- Date: 2026-07-15

Superseded by ADR 0012, which keeps append-only exact-history recovery but
records compaction requests and states in the paired message log instead of a
separate checkpoint file.

ADR 0004 refines only this record's prompt and history-tool placement: recovery
guidance now lives in the initial runtime reminder, normal profiles register the
history tools before their first call, and fixed profiles replace a generic
run-level allowlist. The append-only checkpoint, retrieval, and persistence
decisions below remain accepted.

ADR 0007 further refines guidance placement: the initial reminder omits it, and
active-context assembly emits it only beside an actual compacted-history block.

ADR 0005 further refines the trajectory's physical layout: Chat-compatible
message lines are paired with `message_metadata.jsonl`, where stable refs,
sequence numbers, integrity hashes, and reconstruction layout now live. The
committed trajectory remains append-only and is still the source for compaction
and exact recovery.

ADR 0032 supersedes that physical layout with one self-contained message line.
The append-only compaction and history-retrieval decisions here remain accepted.

## Context

Long tool-using runs need to reduce the messages sent on later model calls, but
the original trajectory is also the evidence used for debugging, inspection,
and future resume behavior. Destructively replacing old messages with a summary
would make exact recovery impossible. Passing an entire transcript location to
the model would also couple retrieval to a writable local filesystem, while
future run storage may be remote or database-backed.

Provider-side compaction is not portable across the supported model adapters.
Picoagent therefore needs a provider-neutral boundary that works with local run
directories now and can support another storage backend later.

## Decision

- `messages.jsonl` remains the append-only source trajectory. Each completed
  message has a stable ref and sequence number; compaction does not edit or
  delete it.
- `compactions.jsonl` is a separate append-only checkpoint log. A checkpoint
  stores the model-generated summary, covered-through ref, first exact ref kept,
  model/provider identity, and reported summary usage.
- When enabled, compaction starts only after the provider has reported input
  usage and the tracked context reaches `trigger_tokens`. The same provider and
  model receive an additional tool-free summary request. The exact recent
  suffix is selected at completed-message boundaries and keeps tool calls with
  their results.
- A later active request contains the original runtime message, the latest
  summary, and the exact recent suffix. Repeated compaction merges the previous
  summary with newly covered messages. The summary is XML-escaped inside its
  synthetic `<compacted-history>` boundary and system instructions classify it
  as historical data rather than a new instruction.
- `history_search` performs Rust-regex search only over messages removed from
  the active context and full textual artifacts linked from their tool results.
  It returns newest matches up to a configurable maximum.
- `history_read` accepts a stable message ref plus bounded `before`/`after`
  counts and returns a conversation-ordered window, expanding tool pairs when
  necessary. Both history tools are read-only and have no cursor.
- The search match cap and artifact preview limit are separate. Reaching the
  match cap omits older matches that are not stored in the returned artifact;
  model-facing output truncation stores the complete already-bounded result as
  a normal artifact.
- No provider/server-side compaction API is implemented by this decision.
- A run does not compact when its tool allowlist removes either history tool or
  both generic artifact inspection tools (`read` and `bash`); retaining the full
  context is safer than removing the exact-recovery path.

## Consequences

- Run directories preserve exact evidence while model requests can remain
  bounded.
- A model can recover exact compacted details without direct transcript write
  access, and a remote store can implement the same trajectory-reader contract.
- Compaction adds provider calls, cost, latency, and summary error risk. A
  failed summary is recorded and leaves the previous/full context available.
- Providers that omit input-token usage cannot trigger automatic compaction.
  `keep_recent_tokens` uses a provider-neutral estimate for boundary selection,
  not the provider tokenizer. Diagnostic reasoning that adapters do not replay
  is excluded from that estimate, while replayable provider items remain.
- Regex retrieval is transparent and dependency-light, but it is not semantic
  search. The model may need to refine a pattern when the match cap is reached.
- The local full-artifact reader uses exact envelope digests, streaming SHA-256
  verification, and `rg` for bounded-memory matching. Reused call ids cannot
  redirect a result to an older artifact; remote readers can provide the same
  interface without that process dependency.
- The launch local message reader materializes one run's trajectory per query;
  the provider-neutral reader boundary permits an indexed backend if run sizes
  later justify it.
- Loaders ignore only an incomplete final JSONL record, and the next append
  repairs that torn tail. Corruption in a completed record remains an error.

## Alternatives Considered

- **Rewrite `messages.jsonl` with the summary.** Rejected because it destroys
  the exact trajectory and weakens crash/resume and audit boundaries.
- **Send the transcript path and rely on `rg`.** Rejected as the only interface
  because it grants more filesystem coupling than retrieval needs and does not
  generalize to remote storage.
- **Use cursored search/read pagination.** Rejected for this version because a
  bounded newest-first search plus ref-centered read and normal artifact spill
  cover the common workflow with a smaller tool contract.
- **Implement semantic/vector retrieval.** Rejected because regex is
  deterministic, inspectable, and sufficient for exact identifiers and known
  facts; vector infrastructure is not yet justified.
- **Use only provider-side compaction.** Rejected because it is not available
  across provider adapters and would not define portable local checkpoints or
  retrieval semantics.

## Related Documents

- [ADR 0004: Stable agent prefix and core history tools](0004-stable-agent-prefix-and-core-history-tools.md)
- [ADR 0005: Chat-compatible message log](0005-openai-chat-compatible-message-log.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Configuration](../configuration.md)
- [Artifact contract](../artifacts.md)
- [Design choices](../design-choices.md)
