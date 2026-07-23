# ADR 0036: Interrupt Agent Activities on Process Restart

- Status: Superseded by ADR 0038
- Date: 2026-07-22
- Supersedes: ADR 0006 (automatic child-activity resume)
- Refines: ADR 0034 (background recovery policy)
- Refines: ADR 0035 (reusable agent activity lifetime)

## Context

A reusable delegated agent has two different lifetimes: its durable thread and
one execution activity. Attempting to continue an activity transparently after
a process crash made the parent task record, child run state, and in-memory
handle describe the same lifecycle fact. Crash windows then required queued
reactivation markers, stopping states, initialization flags, and reconciliation
of an older `final.md` result.

Process crashes are expected to be rare, while model and tool calls can leave
external side effects that no local state machine can classify exactly. The
thread transcript is still valuable after such a failure, but exactly-once
continuation of the interrupted activity is not a required guarantee.

## Decision

- Preserve the reusable agent thread and its complete child checkpoints, but do
  not automatically continue an in-flight activity after process restart.
- A committed agent task in `queued` or `running` becomes `idle` and `paused`.
  Append one sequenced `interrupted` activity output, set the child run to
  `idle`, and retain queued followups and pending steering input.
- Queue a child runtime reminder that the preceding activity stopped after its
  last complete checkpoint and that uncommitted side effects may have occurred.
  The reminder is consumed only if a later explicit `task_send` starts another
  activity.
- Do not launch a child during restart reconciliation and do not reinterpret an
  older child `final.md` as the result of the interrupted activity. The parent
  receives the interruption and decides whether to inspect or retry.
- `task_send` on the paused idle agent clears the pause and reuses the same
  child transcript. No separate resume operation or content-free lifecycle
  message is added.
- Normal in-process completion and failure still move the agent to `idle` and
  may automatically start already queued followups. `task_stop` uses the same
  interruption result and pause semantics without a durable stopping state.
- `queued` remains the initial prepared-run state and the ordinary promoted-tool
  scheduling state. Reactivating an existing idle agent leaves its child idle
  until the runner begins the new activity; queued is not a recovery marker.
- In-flight ordinary background tools remain terminal `interrupted` tasks and
  are never replayed. A half-committed explicit close may still be repaired
  from the child's durable `closed` lifetime state.

## Consequences

- Agent lifetime survives activity completion, failure, stop, and process
  interruption without a second agent object or scheduler.
- Restart recovery has one conservative outcome for active work and does not
  need `stopping`, run initialization flags, or automatic child launch lists.
- A child may have completed its final write just before the parent process
  stopped and still be reported as interrupted. Its transcript remains
  inspectable, but delivery requires an explicit parent decision.
- Explicit retry may repeat external side effects. The interruption notice
  makes that uncertainty visible rather than claiming transparent recovery.
- Pending followups never start merely because the parent process restarted.

## Alternatives Considered

- **Transparently resume every committed child activity.** Rejected because it
  requires cross-file lifecycle reconciliation and cannot provide exactly-once
  external tool behavior.
- **Permanently fail or close the agent after a crash.** Rejected because an
  activity failure does not invalidate the durable transcript or agent
  identity.
- **Add a dedicated `task_resume` operation.** Rejected because an ordinary
  explicit `task_send` both states the next objective and restarts the idle
  child.
- **Automatically run queued followups after restart.** Rejected because it
  hides the interruption and can execute work before the parent inspects
  uncertain side effects.

## Related Documents

- [ADR 0034](0034-atomic-turn-checkpoints.md)
- [ADR 0035](0035-reusable-agent-tasks.md)
- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
