# ADR 0034: Resume from Atomic Turn Checkpoints

- Status: Accepted
- Date: 2026-07-21
- Supersedes: ADR 0027
- Refines: ADR 0006 (resume boundary and child coordination)
- Refines: ADR 0017 (concurrent tool-batch persistence)
- Refines: ADR 0032 (message-log commit grouping)

## Context

Persisting an assistant tool-call message before its results made a normal turn
visible in a half-finished shape. Recovery then needed to classify every missing
result, correlate task records with an unpaired provider call, reconstruct
delegate acknowledgements, and distinguish a resumable child from an ordinary
tool whose side effects were unknown. Compaction had a similar two-append window
between its request and state.

Picoagent runs under a supervisor, cgroup, or container which can terminate the
old process and all locally managed descendants before resume. Workspace and
external side effects may survive, but local futures from the old process do
not. That makes a complete logical turn a simpler recovery boundary than each
individual message.

## Decision

- Each persisted message remains one JSON line with its own `m<N>` ref.
  `_pico.checkpoint` records the group's first ref, zero-based index, and count.
- A normal assistant tool turn commits the assistant message, every tool result
  in provider call order, and any user attachment message as one checkpoint.
- A successful compaction commits its request and assistant state as one
  checkpoint. Initial input, steering input, background delivery, restart
  reminders, and final assistant messages are singleton checkpoints.
- The store serializes a full checkpoint under one writer lock and one
  `write_all`/flush/sync sequence. Readers publish none of a group until all
  newline-terminated lines are present with contiguous refs and metadata. The
  next writer truncates an incomplete tail group before reusing its refs.
- Resume does not synthesize missing tool results. It appends a normal user
  runtime reminder that an uncommitted turn was discarded while its workspace
  or external side effects may have occurred, then lets the model inspect state.
- A task record is recoverable only if the parent log contains its originating
  ToolResult in a complete checkpoint. Pre-checkpoint task files and child runs
  are ignored as orphans. Recognized ordinary background tools become
  `interrupted`; recognized child runs reconcile or resume normally.
- `delegate` prepares the child run directory before returning its task start,
  so every committed child acknowledgement references a self-contained run.
- Resume requires the old picoagent process and its locally managed descendants
  to be dead. A busy child lease is an invariant violation and fails immediately
  rather than being polled. Remote jobs and external effects are outside this
  process-domain guarantee.

## Consequences

- Recovery has no partial assistant/tool shape and no acknowledgement-repair
  path. The originating call id remains useful only to decide whether a task
  record was included in a complete parent checkpoint.
- Child resume remains equivalent to root resume after its queued run starts;
  the parent is still the public recovery entrypoint.
- A model may repeat an operation after inspection, but the harness never
  silently replays an uncommitted side effect.
- Orphan coordination files may remain on disk for audit. They are hidden from
  task controls, and their short task ids are reserved rather than reused.

## Alternatives Considered

- **Keep per-message commits and reconstruct missing results.** Rejected because
  it retains tool-specific crash repair and duplicate-child edge cases.
- **Persist a separate transaction/WAL file.** Rejected because checkpoint
  membership in the self-contained log is enough for one writer.
- **Delete orphan task and child directories on resume.** Rejected because
  ignoring them is safer, preserves audit evidence, and avoids destructive
  cleanup in the recovery path.
- **Wait for a busy child lease.** Rejected because the process-domain
  precondition says no valid old local owner exists during resume.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [ADR 0006](0006-complete-message-resume-and-durable-child-coordination.md)
- [ADR 0017](0017-concurrent-tool-batches-and-explicit-task-controls.md)
- [ADR 0027](0027-correlate-delegate-recovery-with-originating-call.md)
- [ADR 0032](0032-self-contained-message-log.md)
