# Prompt Assets

`agents.yaml` is the typed registry for stable agent-level prose. Every prompt
is a folded `>-` scalar, so YAML removes source-only line wrapping and strips
the final newline before Rust sees the value. `src/prompts.rs` parses the
embedded file once, rejects unknown or empty fields, and exposes the four named
prompts directly.

Runtime assembly, precedence, dynamic values, tool schemas, and execution
contracts remain in Rust. Project `AGENTS.md`, skill metadata, memory paths, and
delegated instructions are dynamic inputs and are not copied into this registry.

Tool descriptions live beside their owning implementations. Standalone base
tools use `src/tools/<tool>/description.md`; subsystem tools remain with their
subsystem. Other stable model instructions live with the subsystem that
assembles them, such as `src/artifact/model-instruction.md`.

These are compile-time assets, not runtime overrides or dynamically discovered
plugins. External executable tools integrate through MCP.

Normal tool descriptions are sent through the provider's sorted tool-schema
field. Core history schemas are present from the first normal call regardless
of the automatic checkpoint trigger. A frozen registry may include delegation,
web, or MCP capabilities selected during run assembly. Memory uses ordinary
file tools and therefore adds no schema. GeneralTask is assigned a delegating
or leaf variant from its remaining depth before it starts; every assembled
profile is then frozen for the run.
