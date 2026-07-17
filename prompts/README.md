# Prompt Assets

Stable model-facing prose lives in Markdown and is embedded into the binary
with Rust `include_str!`. Runtime assembly, precedence, dynamic values, tool
schemas, and execution contracts remain in Rust.

- `agents/system.md`: the invariant system prompt for normal agent calls.
- `agents/compacted-history.md`: stable recovery guidance assembled into the
  synthetic active-context message only when a checkpoint is present.
- `agents/compaction.md`: the separate system prompt for tool-free checkpoint
  summary calls.
- Other files under `agents/`: fixed instructions for named internal profiles.
- Tool descriptions live beside their owning implementations. Standalone base
  tools use `src/tools/<tool>/description.md`; subsystem tools remain with their
  subsystem.
- Other stable model instructions live with the subsystem that assembles them,
  such as `src/artifact/model-instruction.md`.

These files are compile-time assets, not runtime overrides or dynamically
discovered plugins. Project-specific instructions belong in `AGENTS.md`, and
external executable tools integrate through MCP.

Prompt assembly removes source-only soft wrapping inside Markdown paragraphs
while preserving semantic boundaries such as blank lines, headings, list
items, tables, explicit breaks, and fenced code.

Normal tool descriptions are sent through the provider's sorted tool-schema
field. Core history schemas are present from the first normal call regardless
of the automatic checkpoint trigger. A frozen registry may include memory,
delegation, web, or MCP capabilities selected during run assembly. GeneralTask
is assigned a delegating or leaf variant from its remaining depth before it
starts; every assembled profile is then frozen for the run.
