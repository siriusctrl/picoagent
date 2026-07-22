# ADR 0035: Model Delegated Agents as Reusable Tasks

- Status: Accepted
- Date: 2026-07-22
- Refines: ADR 0010 (agent activity, stopping, and lifetime)
- Refines: ADR 0017 (task-control surface and wait semantics)
- Refines: ADR 0034 (recovery and repeated result delivery)

ADR 0036 refines the restart behavior below: a process restart interrupts the
current activity and leaves its reusable thread idle and paused instead of
automatically launching the child.

## Context

A delegated child and a background task shared scheduling, status, waiting,
inspection, persistence, and parent ownership, but the interface still treated a
child's final answer as the end of both its current work and its identity. That
forced a new child for related follow-up work and made its prior transcript
unavailable as working context. A single `task_steer` operation also could not
express whether a message should influence work already in progress or wait
until that work had produced a result.

Waiting for every selected task delayed useful synthesis after the first result
was ready. Conversely, making a follow-up call block until the child became idle
would stall the parent for no semantic reason.

## Decision

- Treat a delegated agent as the reusable child-backed subtype of the existing
  task abstraction. There is still one `TaskManager`, one `AgentRunner`, and one
  task-control family; there is no separate agent manager or scheduler.
- `delegate` creates one isolated child run and starts its first activity. An
  activity result moves the task and child run to `idle` instead of completing
  the agent. The child transcript and artifacts remain its durable context.
- Replace `task_steer` with `task_send`, whose required `mode` is `steer` or
  `followup`. Both are ordinary user messages to the child:
  - `steer` on an active agent enters the child's pending-input log and becomes
    visible after the current complete assistant/tool batch.
  - `followup` on an active agent stays durably queued in the parent task record.
    After the current activity result, queued followups move to the child's
    pending-input log in order and the same child starts automatically.
  - Either mode on an idle agent starts that agent immediately.
- Keep the model-facing distinction about message intent, not provider or
  internal “turn” mechanics. The runtime owns safe checkpoint placement.
- Store immutable agent activity outputs as a contiguous sequence. Result
  notices include `output_seq`; recovery derives the highest delivered sequence
  for each task from the parent transcript, so one agent can report repeatedly
  without a second delivery ledger.
- Make `task_wait` wait for any selected task to become inactive or for its
  bounded interval to expire. It returns the full selected status snapshot, so
  other entries may still be running. A newly ready activity output also wakes a
  wait even when a queued followup has already made that same agent active again.
- Add `task_list` for every delegated agent directly owned by the current run.
  Idle agents remain visible; closed agents are optional. Keep short task ids
  parent-local instead of adding a global agent registry or cross-tree control.
- Separate activity stopping from agent lifetime. `task_stop` interrupts an
  active agent after its last complete checkpoint and leaves it idle, paused,
  and reusable. Queued input is retained but cannot autoactivate until the next
  explicit `task_send`, which clears the pause. `task_close` is the explicit
  permanent operation and accepts only an idle agent, discarding any queued
  followups. Agents are not closed automatically.
- Reactivation persists the parent task as `running` and launches the existing
  idle child. Close and activation serialize through the same task-record lock,
  and recovery repairs a child close committed just before its parent task
  record. Process restart does not launch a queued or running activity; ADR
  0036 defines its interruption and explicit-retry boundary.
- Ordinary tools promoted after the foreground window remain one-shot tasks and
  retain their existing terminal states. Agent-only controls reject them.

## Consequences

- Related work can reuse one isolated transcript without copying parent context
  or creating a second agent identity.
- A parent can queue follow-up work and continue immediately. Several followups
  queued during one activity are delivered together, in order, at the next safe
  activity boundary.
- `idle` means the agent exists but consumes no child execution slot. `closed`
  is durable and rejects later input. Unclosed idle runs remain as inspectable,
  portable run output by default.
- `paused` is orthogonal to lifetime: it prevents queued input from restarting
  an idle agent after a stop, but the next explicit send reuses the same child.
- Parent delivery and active-task reminders can describe different aspects of
  the same agent at once: output sequence N may be ready while activity N+1 is
  already running.
- Runtime events likewise distinguish child activity start, completion,
  failure, and stopping from the separate explicit agent-close event. Root run
  completion and failure retain their terminal meaning.
- The task record format and child run state gain `idle` and `closed`; agent
  records gain queued followups and multiple outputs. Recovery admits only
  committed parent-owned work, interrupts active activities, and preserves the
  atomic checkpoint boundary.
- “List all agents” is deliberately scoped to the current run's direct agents.
  Descendants are listed and controlled by their own parent run, avoiding
  ambiguous short ids and a second global ownership model.

## Alternatives Considered

- **Create separate agent and task managers and duplicate controls.** Rejected
  because a child agent already uses the task scheduler, persistence, and
  supervision lifecycle; the split adds schema and recovery branches without a
  behavioral boundary.
- **Infer steer versus followup from whether the agent is currently running.**
  Rejected because the caller may intentionally queue later work while current
  work continues. Blocking until idle would unnecessarily stall the parent.
- **Expose current-turn and next-turn operations.** Rejected because those are
  provider-facing implementation terms. `steer` and `followup` state intent
  while the runtime chooses a safe complete-checkpoint boundary.
- **Automatically close an agent after every final answer.** Rejected because
  it makes related follow-up work create a new transcript and conflates an
  activity result with lifetime management.
- **Wait until every selected task settles.** Rejected because the first ready
  result may unblock useful parent work while independent tasks continue.
- **Use one global cross-tree agent registry.** Rejected because parent-local
  ownership and ids already make recovery and authorization unambiguous.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
- [ADR 0010](0010-parent-controlled-background-work.md)
- [ADR 0017](0017-concurrent-tool-batches-and-explicit-task-controls.md)
- [ADR 0034](0034-atomic-turn-checkpoints.md)
- [ADR 0036](0036-interrupt-agent-activities-on-restart.md)
