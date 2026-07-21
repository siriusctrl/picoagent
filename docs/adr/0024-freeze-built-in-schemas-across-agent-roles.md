# ADR 0024: Freeze Built-In Schemas Across Agent Roles

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0004 (agent-role schema stability)
- Refines: ADR 0017 (delegate availability)

ADR 0028 refines only guidance placement below. Stable cross-tool workflow now
lives in the shared system prompt; concrete role, delegation depth, and active
task state remain runtime-reminder data.

## Context

Root, delegating GeneralTask, and leaf GeneralTask were durable capability
profiles, but leaf runs omitted `delegate`. That made a child's tool prefix
depend on remaining delegation depth even though the runtime could reject an
out-of-depth call directly. It also made otherwise identical parent and child
requests less likely to reuse provider prefix caches.

The distinct stored profiles still matter for recovery: a child created as a
leaf must not become delegating merely because configuration changes before
resume. The model-facing role guidance and exact current depth are dynamic run
facts rather than reasons to vary the stable system prompt or built-in schemas.

## Decision

- Keep `root`, `general_task_delegating`, and `general_task_leaf` as explicit
  persisted profiles.
- Assemble the same built-in schemas for every profile. This includes
  `delegate`, both history tools, and all task controls. Optional startup
  integrations such as web search and MCP remain configuration-dependent.
- Persist exact `remaining_delegation_depth` in run records and the child's
  exact remaining depth in delegated task records. Show the run value in the
  initial runtime reminder.
- At remaining depth zero, `delegate` returns a normal local tool error before
  creating a task or child run. Schema membership does not represent runtime
  depth.
- Present both GeneralTask profiles to the model as the common
  `profile: general_task` role. Put concise child-role guidance and dynamic
  tool-use hints in the runtime reminder; keep the one system prompt limited to
  universal operating and cross-tool rules.
- Keep tool-specific mechanics in each `tool.yaml`. Runtime guidance may
  explain relationships between a tool family, but does not restate `read`,
  web-search, or other individual schemas.
- Bump pre-release run and task record formats to version 7. Older records are
  intentionally not migrated.

## Consequences

- Root, delegating GeneralTask, leaf GeneralTask, and compaction requests can
  share byte-identical built-in system/tool prefixes for one startup
  configuration.
- A leaf sees `delegate` even though calling it fails. The reminder makes the
  exact depth explicit and the error is recoverable like any other tool error.
- Resume preserves execution authority independently of current
  `max_subagent_depth`, while the stored delegating/leaf identity remains an
  additional consistency check.
- Runtime reminders vary by role and depth at the message tail. This preserves
  stable prefix reuse without hiding current capabilities from the model.
- Run and task record version 6 artifacts cannot be resumed by this pre-release
  harness.

## Alternatives Considered

- **Remove `delegate` from leaf schemas.** Rejected because depth is dynamic
  execution state and the membership difference invalidates a larger cache
  prefix.
- **Collapse GeneralTask profiles into one stored profile.** Rejected because
  the explicit delegating/leaf recovery identity is a repository invariant and
  provides a useful consistency check.
- **Put role and depth in the system prompt.** Rejected because those values
  vary per run and would shorten reusable provider prefixes.
- **Duplicate every tool description in runtime guidance.** Rejected because
  static `tool.yaml` manifests are authoritative and duplicated prose drifts.
- **Derive remaining depth from current configuration on resume.** Rejected
  because changing configuration could silently change an interrupted run's
  authority.

## Related Documents

- [ADR 0004: Keep the normal agent prefix and core history tools stable](0004-stable-agent-prefix-and-core-history-tools.md)
- [ADR 0017: Concurrent tool batches and explicit task controls](0017-concurrent-tool-batches-and-explicit-task-controls.md)
- [Architecture](../architecture.md)
- [Runtime model](../runtime-model.md)
- [Prompt assets](../../prompts/README.md)
