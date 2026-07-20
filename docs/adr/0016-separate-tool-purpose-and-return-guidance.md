# ADR 0016: Separate Tool Purpose from Return Guidance

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0015 (local manifest prose fields)

## Context

The first local `tool.yaml` format put all model-facing prose in one
`description`. Tools with structured or action-dependent results mixed their
purpose, usage constraints, return fields, and follow-up instructions into one
long scalar. That was legal provider input but unnecessarily difficult to edit
and review.

OpenAI-compatible and Anthropic-compatible tool definitions expose one tool
description and an input schema; they do not provide a portable return-guidance
field. The source format can still separate authoring concerns as long as the
provider contract remains standard and deterministic.

## Decision

- Every local `tool.yaml` has exactly `name`, `description`, `returns`, and
  `input_schema`.
- `description` covers purpose, when to call the tool, side effects, and
  constraints. `returns` covers the successful logical result shape, field
  meanings, interpretation, and tool-specific follow-up.
- Both prose fields are required folded scalars and receive the same non-empty,
  boundary-whitespace validation.
- The manifest loader produces the standard model-facing description as
  `<description>\n\nReturns: <returns>` before registry sorting, freezing,
  hashing, or provider serialization.
- Generic artifact spill, truncation, and error behavior remains shared runtime
  guidance rather than being repeated in each `returns` field.
- `returns` is an authoring field, not a private provider extension or a formal
  output JSON Schema. MCP tools retain their server-provided descriptions.

## Consequences

- Maintainers can revise tool purpose and result contracts independently.
- The model still receives one ordinary provider description with an explicit
  semantic boundary and no protocol change.
- Every local tool must state its successful result, even if that statement is
  only one sentence.
- The composed description and therefore the frozen schema hash change when
  either source field changes, as expected.

## Alternatives Considered

- **Keep one authored description.** Rejected because structured return
  explanations were obscuring the tool's purpose and usage guidance.
- **Make returns optional.** Rejected because a uniform local contract is easier
  to review and prevents output guidance from drifting back into description.
- **Add output_schema.** Rejected because providers do not consume it, many
  results are text or JSONL, and shared artifact handling can change the
  model-facing envelope.
- **Send a private returns field to providers.** Rejected because it would no
  longer be an OpenAI-compatible or Anthropic-compatible tool definition.

## Related Documents

- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
- [ADR 0015](0015-local-tool-yaml-manifests.md)
