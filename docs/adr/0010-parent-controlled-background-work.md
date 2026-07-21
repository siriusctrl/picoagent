# ADR 0010: Use Parent-Controlled Background Work Without Step or Execution Caps

- Status: Accepted
- Date: 2026-07-17
- Refines: ADR 0006 (child control, cancellation, and message delivery)

ADR 0017 later refines how work enters this lifecycle and how controls are
exposed: one assistant batch now runs concurrently under a shared foreground
window, only unfinished direct futures are promoted, `delegate` starts child
agents, and five static task-control tools replace the action union. The
no-step-cap, exact-future, durable-control, and recovery decisions here remain
accepted.

ADR 0020 later unifies running and terminal model-facing notices and makes every
terminal background result artifact-only. The parent-control decision remains
accepted.

ADR 0030 later replaces that artifact-only presentation with the ordinary
per-result inline/preview/artifact policy. Parent-controlled delivery remains
accepted.

## Context

The original runtime bounded Root and GeneralTask runs with model-step counts and
gave direct tools, background tools, and child agents hard execution timeouts.
Those limits stop pathological work, but they also turn an arbitrary harness
number into a false task failure. Raising a subagent cap from 8 to 100 only moves
that boundary. Killing a direct tool when its foreground deadline expires also
loses useful work that could continue independently.

Parents already receive durable task ids and terminal result messages. They need
enough observation and control to decide whether a live child should continue,
change direction, or stop. Child transcripts must remain separate rather than
being copied wholesale into the parent context.

## Decision

- Root and GeneralTask loops have no model-step cap. Model request deadlines,
  provider errors, explicit cancellation, and process failure remain real run
  boundaries.
- Direct ordinary tools start in the foreground. If the configured foreground
  window expires, picoagent preserves the same in-flight future, records it as
  a running background task, and returns an acknowledgement with its task id.
  It does not stop or restart the tool.
- Explicitly spawned tools and child agents have no hard harness execution
  deadline. Restart recovery still marks an in-flight ordinary tool
  `interrupted`, because its external side effects cannot be reconstructed.
- One `task` tool owns `status`, bounded `wait`, `inspect`, `steer`, and `stop`.
  Inspect returns the latest six durable child messages by default in native
  Chat-compatible form and pages backward with an exclusive `before_seq`.
- Steer is non-interrupting. It durably queues an ordinary user message after
  the child's current completed assistant response and complete tool-call batch,
  before the next provider request. It never fabricates a tool result or breaks
  a provider tool-call/result pair.
- Stop aborts only the selected task and persists `cancelled`; child run state is
  cancelled too. A parent cannot silently finish with live tasks: the runner
  waits for one bounded task interval, then returns current task state to the
  model if work is still active.

## Consequences

- Long but healthy tasks are not failed by arbitrary step or execution counts.
- A foreground timeout changes scheduling and model visibility, not execution
  outcome. Terminal output still uses the normal artifact and background-result
  contracts.
- Parents can inspect and redirect children without injecting complete child
  transcripts into every parent request.
- A model can still loop indefinitely. Operators retain provider request
  deadlines and external process/job cancellation; picoagent does not pretend
  an arbitrary model-call count proves failure.
- Pending steering input is append-only and idempotently committed into the
  child's ordinary message trajectory. This adds one small per-run input log.

## Alternatives Considered

- **Raise subagent max steps to 100.** Rejected because it preserves an arbitrary
  failure boundary and separate Root/child semantics.
- **Keep hard background execution timeouts.** Rejected because a parent with
  status and stop can own that decision, while useful independent work should
  not be destroyed automatically.
- **Interrupt the current assistant/tool batch when steering arrives.** Rejected
  because it complicates cancellation and can leave invalid provider tool-call
  sequences. An explicit stop remains available when interruption is intended.
- **Copy the child transcript into the parent.** Rejected because it duplicates
  durable state and grows the parent context. Bounded inspect is sufficient.
- **Expose separate wait, inspect, steer, and stop tools.** Rejected in favor of
  one small task-control schema.

## Related Documents

- [ADR 0006: Complete-message resume and durable child coordination](0006-complete-message-resume-and-durable-child-coordination.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Configuration](../configuration.md)
- [Design choices](../design-choices.md)
