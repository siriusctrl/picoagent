# ADR 0027: Correlate delegate recovery with its originating call

- Status: Superseded by ADR 0034 and ADR 0038
- Date: 2026-07-21
- Refines: ADR 0006 (delegate crash-window recovery)

ADR 0034 supersedes this acknowledgement-repair decision. The originating call
id now admits a task only when its ToolResult belongs to a complete parent
checkpoint; an uncommitted task is an ignored orphan.

## Context

`delegate` durably creates and starts an agent task before its ordinary tool
result is appended to the parent transcript. A process can therefore stop after
the task record commits but before the status-less acknowledgement fills the
provider's `tool_call_id` slot.

Agent task records previously omitted that originating call id. On resume the
runner treated the unpaired `delegate` call like an unknown interrupted direct
tool, while task reconciliation independently resumed the already-created
child. The parent could then receive both an interrupted error for the call and
the child result for work which was not actually interrupted.

## Decision

- Persist the originating provider call id on both promoted direct-tool and
  delegated-agent task records. Advance the pre-release task record to version
  9 and require a non-empty id for both kinds.
- Keep that id internal. Model-facing start and terminal notices remain
  correlated by the short run-local task id.
- When resume finds an unpaired `delegate` call, match it to the undelivered
  agent task by originating call id and kind. Append the same status-less
  acknowledgement that the original call would have returned, then resume the
  existing child instead of replaying `delegate`.
- Continue deriving acknowledgement and terminal-result delivery from committed
  parent messages. A later resume therefore cannot append either one twice.

## Consequences

The crash window has one recovery authority: the durable task identifies the
already-created child and the parent transcript identifies what the provider
has observed. Child execution remains resumable, while non-resumable ordinary
tools retain their conservative interrupted behavior.

Task record version 8 is intentionally not accepted by this pre-release
internal harness. There is no compatibility migration for unused historical
runs.

## Alternatives Considered

- **Treat the missing acknowledgement as an interrupted error.** Rejected
  because the durable child is resumable and may already have completed.
- **Replay `delegate`.** Rejected because it creates a second child for the same
  provider call.
- **Expose the provider call id in runtime notices.** Rejected because task
  controls and terminal delivery use the shorter run-local task id.
- **Persist separate acknowledged and delivered booleans.** Rejected because a
  crash between message append and flag update creates two authorities.

## Related Documents

- [ADR 0006](0006-complete-message-resume-and-durable-child-coordination.md)
- [Architecture](../architecture.md)
