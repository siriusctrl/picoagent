# ADR 0028: Keep Cross-Tool Workflow in the Stable System Prompt

- Status: Accepted
- Date: 2026-07-21
- Refines: ADR 0004 (history guidance placement)
- Refines: ADR 0024 (tool-family guidance placement)

## Context

The initial runtime reminder repeated a fixed task-control paragraph on every
run, while compacted-history and graph workflow could be restated in benchmark
task prompts to force coverage. Those rules describe picoagent itself, not a
particular workspace or task. Keeping stable prose in Rust or user requests
makes prompt ownership unclear and makes realistic trajectory evaluation less
representative.

Complete tool mechanics still belong in each typed `tool.yaml`; copying every
argument and result contract into the system prompt would waste tokens and
create two authorities. Concrete role, delegation depth, active tasks,
workspace paths, skills, and memory locations remain dynamic.

## Decision

- Put concise, universal relationships for task control, compacted-history
  recovery, and file-backed planning graphs in the invariant system prompt in
  `prompts/agents.yaml`.
- Keep complete purpose, argument, return, and local constraint prose in each
  tool's `tool.yaml`.
- Keep concrete run facts in runtime reminders. Remove the fixed
  `<tool-guidance>` section from initial reminder assembly.
- Retain a small continuation reminder only beside an active compacted state;
  it identifies the reordered boundary rather than repeating tool usage.
- Do not append harness workflow to ordinary user tasks. Stress tests should
  create a task that naturally needs the capability and observe whether the
  model follows the stable guidance.

## Consequences

- Root and GeneralTask share one clearer, byte-stable system prefix, while the
  initial user message becomes smaller and contains only run-specific context.
- Normal runs pay a small cached system-token cost for the family mental model,
  matching their always-present built-in schemas.
- Persisted user messages no longer make stable harness policy look like part
  of the user's business request.
- Tool manifests remain the sole authority for signatures and result shapes.

## Alternatives Considered

- **Keep fixed guidance in every runtime reminder.** Rejected because it is not
  runtime state and obscures the stable/dynamic boundary.
- **Put all guidance only in tool descriptions.** Rejected because no one tool
  owns relationships spanning delegation, task controls, graph files, and
  history retrieval.
- **Duplicate complete tool contracts in the system prompt.** Rejected because
  it spends tokens and can drift from typed manifests.
- **Force tool calls through benchmark user prompts.** Rejected because it
  tests instruction compliance rather than whether picoagent's own guidance is
  sufficient.

## Related Documents

- [ADR 0004: Stable normal prefix and core history tools](0004-stable-agent-prefix-and-core-history-tools.md)
- [ADR 0024: Frozen built-in schemas](0024-freeze-built-in-schemas-across-agent-roles.md)
- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
