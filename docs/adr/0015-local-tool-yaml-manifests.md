# ADR 0015: Package Local Tool Contracts as Typed YAML Manifests

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0014 (local tool contract packaging)

ADR 0016 later refines the authored prose fields: local manifests separate
purpose in `description` from result guidance in `returns`, then compose both
into the standard provider description. The other decisions remain accepted.

ADR 0017 later removes the sole runtime schema augmentation that this record
allowed. With `spawn` removed, every local manifest now describes one complete
static schema; profile selection changes membership only.

ADR 0019 later groups related adapter directories. Each leaf manifest still
declares the complete provider-visible name; paths do not derive names.

ADR 0024 later removes agent depth as a built-in membership choice. Root and
both GeneralTask profiles now expose the same complete local schemas; optional
startup integrations can still add complete specs.

ADR 0049 replaces server-provided dynamic MCP schemas in the provider prefix
with one fixed local `mcp` manifest. Exact remote schemas now remain in
artifact `catalog.json` files outside model context.

## Context

ADR 0014 put every local model-facing adapter in one predictable directory,
but its static contract was still split between a Markdown description and a
Rust `json!` schema. Reading or editing the model-visible contract required
switching formats and locating the relevant method inside implementation code.
For this internal harness, direct discoverability and easy review matter more
than keeping static JSON Schema construction in Rust.

Rust still must own executable behavior and the few values that depend on an
assembled run. A manifest should make the static contract clearer without
becoming a plugin format, template language, or second execution contract.

## Decision

- Each local adapter directory contains one typed, compile-time `tool.yaml`
  with exactly `name`, `description`, and `input_schema` fields.
- Descriptions use folded YAML scalars so source wrapping does not introduce
  model-visible newlines. The common parser rejects unknown fields, empty or
  boundary-padded names and descriptions, and schemas whose root is not an
  object.
- Rust continues to own argument deserialization, semantic validation,
  execution, registry assembly, and any runtime-dependent schema change.
- `spawn` is the sole dynamic local schema. Its manifest describes the complete
  static superset; Rust injects the exact sorted spawnable-tool enum and removes
  tool-only fields for profiles that cannot spawn tools.
- MCP adapters keep their server-provided dynamic names, descriptions, and
  schemas. They do not use local manifests.
- Manifests are embedded into the binary. They are not runtime overrides, a
  centralized global catalog, or a public plugin ABI.

## Consequences

- A maintainer can review a local tool's complete static model contract in one
  short file beside the implementation.
- Schema-only and wording changes no longer require editing Rust JSON literals.
- Tests reject malformed checked-in manifests; registration of a malformed
  embedded manifest fails startup. Package verification must include every
  manifest.
- The Rust argument type and JSON Schema remain deliberately separate boundary
  representations; focused tests are required to prevent behavioral drift.
- Runtime flexibility is intentionally limited to explicit Rust augmentation,
  avoiding a generic interpolation or schema-generation layer.

## Alternatives Considered

- **Keep Markdown descriptions and Rust schemas.** Rejected because the split
  made the most frequently reviewed contract harder to understand and edit.
- **Use one repository-wide tool registry YAML.** Rejected because it separates
  contracts from their owners and creates a high-conflict central file.
- **Generate Rust arguments or validation from JSON Schema.** Rejected because
  code generation adds machinery without removing the need for semantic checks.
- **Add placeholders or a manifest expression language.** Rejected because one
  explicit `spawn` augmentation is smaller and easier to audit.

## Related Documents

- [Prompt assets](../../prompts/README.md)
- [Architecture](../architecture.md)
- [Source map](../source-map.md)
- [Design choices](../design-choices.md)
- [ADR 0014](0014-flat-tool-adapters-and-explicit-assembly.md)
