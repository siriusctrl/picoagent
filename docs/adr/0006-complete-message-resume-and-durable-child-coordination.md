# ADR 0006: Resume at complete-message boundaries and keep child coordination separate

- Status: Accepted
- Date: 2026-07-16
- Refines: ADR 0001 (resume behavior)

## Context

Main runs need to survive process loss. Subagents use the same runner and have
their own durable messages, so treating them as disposable background futures
would create a different execution model. At the same time, automatically
replaying an interrupted shell command or other tool can duplicate external
side effects. The parent also needs to learn each terminal child result exactly
once without copying a second transcript into task records.

## Decision

- Resume a non-completed run from its last committed complete message and latest
  compaction checkpoint.
- Persist the run profile, depth, parent id, prompt, and delegated instructions
  needed to rebuild the same capability profile.
- Hold a filesystem execution lease for the lifetime of a new or resumed loop.
- Require the same non-secret provider wire fingerprint on resume; credentials
  are excluded from it.
- When a completed assistant message has an unpaired direct tool call, append an
  `is_error` tool result that says execution was interrupted and side effects
  are unknown. Never execute that call automatically.
- Keep each durable GeneralTask transcript in its own child run and resume that
  child through its parent with the same `AgentRunner`.
- Keep task JSON as parent-child coordination state: kind, state, child id,
  prompt, timeout, result, and error. Derive terminal-result delivery from
  committed `BackgroundTaskResult.task_id` entries in the parent transcript.
- Mark in-flight ordinary background tools `interrupted` after restart. Reconcile
  completed/failed child runs and resume queued/running children.
- Hold a cancellation guard while a run owns background work. A dropped future
  aborts in-memory descendants but leaves durable task state for the next lease
  owner; explicit failure paths settle state before releasing the lease.

## Consequences

Runs can continue without provider-stream replay or a second agent class. Tool
side effects are conservative and visible to the model. Parent and child run
directories remain portable, self-contained outputs. Resume requires the same
workspace, provider wire configuration, model, and compatible current
capability configuration.

A process can stop after the tool itself changed external state but before its
result committed. Picoagent cannot infer that outcome; the explicit interrupted
message makes that uncertainty part of the trajectory.

## Alternatives Considered

- **Replay every missing tool result.** Rejected because writes, shell commands,
  network calls, and MCP tools may not be idempotent.
- **Do not resume durable GeneralTask subagents.** Rejected because those
  children have normal run transcripts and task records. Synchronous
  MemoryMaintenance children remain direct-tool work and are not covered by
  this guarantee.
- **Store child messages in task JSON.** Rejected because it duplicates the
  child transcript and creates two recovery authorities.
- **Persist a `delivered` flag.** Rejected because a crash between message append
  and flag update creates disagreement; the committed parent message is the
  delivery fact.
- **Introduce a workflow database.** Rejected for the current single-host,
  file-based harness; the run lease and task records cover the required state.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [ADR 0001](0001-durable-messages-transient-stream-deltas.md)
- [ADR 0005](0005-openai-chat-compatible-message-log.md)
