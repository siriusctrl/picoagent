# ADR 0033: Isolate Delegated Context

- Status: Accepted
- Date: 2026-07-21
- Supersedes: ADR 0025

## Context

Fork delegation copied a frozen parent trajectory and referenced artifacts into
each child. That choice spread context mode and fork-boundary state across the
delegate schema, run and task records, child launch and recovery, compaction
projection, artifact resolution, prompts, and tests. It also made the child
model choice depend on the parent.

Picoagent is an internal harness whose parent and child already share a
workspace. A parent can provide durable context through explicit paths and can
put the task-specific facts in the delegated prompt. The extra inherited
conversation mode is not valuable enough to justify its cross-module state.

## Decision

- Every `delegate` starts an isolated GeneralTask child. The model-facing input
  contains only `name` and `prompt`; there is no context-mode selector.
- The delegated prompt must contain the complete objective and task-specific
  context. Parent conversation, compaction records, pending-input ids, and
  artifact references are not copied.
- Every child uses the configured GeneralTask model and output limits.
- A child owns an ordinary self-contained trajectory and resumes through the
  same `AgentRunner` path as other runs. Parent-child coordination and terminal
  result delivery remain durable.
- Run and task records no longer persist delegated context or fork boundaries.
  Their pre-release versions advance to 10.
- Artifact verification accepts only references owned by the current run.

## Consequences

- Delegate launch and child resume no longer branch on context mode.
- Compaction has no delegated-assignment pinning or first-fork-request case.
- Artifact lookup no longer copies or resolves ancestor-owned bytes.
- Parents must write self-contained child prompts and point to workspace files
  when more context is needed.
- Existing pre-release run/task records are not compatibility targets.

## Alternatives Considered

- **Keep fork but isolate it to child creation.** Rejected because compaction
  and immutable artifact references would still retain fork-specific behavior.
- **Keep both modes behind an internal option.** Rejected because the dormant
  state still expands persistence and recovery contracts.
- **Make every child inherit parent context.** Rejected because unrelated work
  would pay the context and coupling cost by default.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [ADR 0006](0006-complete-message-resume-and-durable-child-coordination.md)
- [ADR 0017](0017-concurrent-tool-batches-and-explicit-task-controls.md)
