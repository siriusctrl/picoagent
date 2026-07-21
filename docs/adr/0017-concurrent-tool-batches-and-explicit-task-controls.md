# ADR 0017: Concurrent Tool Batches and Explicit Task Controls

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0010 (background entry and control surface)
- Refines: ADR 0014 (tool assembly and capability policy)

ADR 0020 later refines the model-facing lifecycle protocol: starts and terminal
delivery now use one background-task runtime tag, `delegate` accepts a display
name, and terminal bodies contain only complete artifact paths. The concurrent
batch, foreground-window, task-control, and recovery decisions here remain
accepted.

ADR 0022 later specifies that native image inputs follow the complete ordered
tool-result batch in one user message. The concurrency and correlation decisions
here remain accepted.

ADR 0024 later makes `delegate` present in every Root and GeneralTask schema.
Remaining depth is persisted runtime state, and a depth-zero call fails locally
before task creation. The task-control and asynchronous execution decisions
here remain accepted.

## Context

The runner executed tool calls from one assistant message sequentially. This
wasted latency when the model intentionally returned independent reads, tests,
or other operations together. The single `spawn` schema also combined two
different concepts: explicitly backgrounding an ordinary tool and delegating a
GeneralTask child. The `task` schema then combined five unrelated operations
behind an `action` discriminator.

Those unions made several fields conditional on another field, obscured which
operation the model intended, and forced runtime schema augmentation for the
ordinary-tool allowlist. Making every direct action asynchronous would avoid a
foreground distinction, but would add a task-status round trip to common
dependent sequences such as read, decide, then write.

## Decision

- All direct tool calls in one assistant message start concurrently and share
  one configured foreground window. The default window is 30 seconds.
- If every call settles before the window ends, the batch returns immediately.
  Completion events may retain actual completion order, while durable tool
  messages are committed in the assistant's original call order.
- Each immediate result remains correlated with its original provider
  `tool_call_id`. At the window boundary, only unfinished exact futures are
  promoted into durable task records; they are not stopped or restarted.
  The runner first resumes and tracks every unfinished future, then announces
  promotions in original call order so one parked future cannot deadlock
  another promotion's event write.
  Each immediate acknowledgement remains the result for the original call and
  contains a short run-local `task_id`; it is not terminal. A later background
  message is correlated by that task id, while any artifact metadata retains
  the originating provider call id.
- Resume reconstructs a missing acknowledgement from a matching durable
  promoted-task record before it delivers that task's terminal result.
- The stable system prompt says calls in one assistant message execute
  concurrently and tells the model to batch only independent operations. The
  harness does not infer dependencies or serialize side effects on the model's
  behalf.
- Remove explicit ordinary-tool spawning. An ordinary tool enters the task
  lifecycle only through automatic foreground promotion.
- `delegate({prompt})` is the sole asynchronous GeneralTask start operation.
  It is present only in Root and depth-eligible GeneralTask profiles.
- Replace the action-discriminated `task` tool with complete static
  `task_status`, `task_wait`, `task_inspect`, `task_steer`, and
  `task_stop` adapters. All profiles retain these controls because direct
  tools can be promoted in any profile.
- The existing task semaphore limits delegated children. It does not serialize
  the initial direct batch or pause already-running futures after promotion.

## Consequences

- Independent calls use one latency window instead of summing their individual
  durations. A mixed batch can return fast results and task references for slow
  calls after the same deadline.
- Provider tool-call/result ordering stays deterministic even when runtime
  completion order differs, and background correlation no longer overloads a
  provider call id.
- Tool schemas become smaller and entirely static. Profiles differ by complete
  tool membership rather than dynamic fields within a union schema.
- Independent side-effecting calls can race. Prompt guidance and the original
  result order make the contract explicit, but the model remains responsible
  for issuing dependent operations in separate turns.
- More task-control names appear in the tool list, trading a slightly larger
  schema-name surface for simpler arguments, validation, documentation, and
  model selection.

## Alternatives Considered

- **Keep sequential direct execution.** Rejected because independent batches
  unnecessarily add wall-clock latency.
- **Give every call its own consecutive foreground timeout.** Rejected because
  a batch of slow calls could wait for the timeout multiplied by its size.
- **Return results in completion order.** Rejected because it makes durable
  trajectories and provider pairing harder to reason about.
- **Make every ordinary action asynchronous.** Rejected because common
  dependent operations would require extra task-control model turns.
- **Keep `spawn(kind=tool|agent)` and `task(action=...)`.** Rejected because
  conditional fields and runtime schema rewriting make the small internal
  harness harder to understand and maintain.

## Related Documents

- [ADR 0010: Parent-controlled background work](0010-parent-controlled-background-work.md)
- [ADR 0014: Flat tool adapters and explicit assembly](0014-flat-tool-adapters-and-explicit-assembly.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Configuration](../configuration.md)
