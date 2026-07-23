# ADR 0039: Keep Feature Guidance with Capabilities

- Status: Accepted
- Date: 2026-07-23
- Supersedes: ADR 0028 (cross-tool workflow in the system prompt)
- Refines: ADR 0024 (stable prompt and frozen schemas)

## Context

The shared system prompt named built-in tools and explained several feature
workflows. That was stable for a fixed product surface, but it became a second
source of prompting influence when evaluating a build with one tool removed.
Removing a schema did not remove the model's instructions to use that feature.

The typed tool manifests already own model-facing purpose, return guidance, and
input schemas. Dynamic reminders and tool results already own concrete run
facts such as delegation depth, memory paths, and active handles.

## Decision

- Keep the invariant system prompt limited to tool-agnostic operating behavior:
  intent boundaries, evidence, schema authority, modality availability,
  concurrent-call semantics, instruction precedence, process permissions, and
  final reporting.
- Put feature workflow in the corresponding typed `tool.yaml`. A manifest may
  describe when its capability is useful and how to interpret its result.
- Refer to optional sibling capabilities generically unless their exact name is
  intrinsic to the current tool's contract.
- Keep concrete feature state in dynamic reminders and runtime results.
- Test that the stable system prompt does not name local tools.

## Consequences

- Tool ablation removes both the schema and the feature's static guidance.
- Each manifest must be understandable without assuming that every sibling tool
  is present.
- A small amount of relationship prose may repeat across related manifests,
  but each installed capability remains self-describing.
- Root, GeneralTask, and compaction still share one stable system prompt and
  frozen schema set per run.

## Alternatives Considered

- **Keep cross-tool workflow in the system prompt.** Superseded because it
  leaks guidance for ablated capabilities.
- **Generate a different system prompt from installed tools.** Rejected because
  it creates another schema-to-prompt assembly layer and weakens prefix
  stability.
- **Remove workflow guidance entirely.** Rejected because non-obvious
  capabilities still need model-facing usage guidance.

## Related Documents

- [ADR 0024](0024-freeze-built-in-schemas-across-agent-roles.md)
- [ADR 0028](0028-stable-cross-tool-workflow.md)
- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
