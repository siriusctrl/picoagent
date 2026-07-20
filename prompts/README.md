# Prompt Assets

`agents.yaml` is the typed registry for stable agent-level prose. Every prompt
is a folded `>-` scalar, so YAML removes source-only line wrapping and strips
the final newline before Rust sees the value. `src/prompts.rs` parses the
embedded file once, rejects unknown or empty fields, and exposes the three named
prompts directly.

Runtime assembly, precedence, dynamic values, tool schemas, and execution
contracts remain in Rust. Project `AGENTS.md`, skill metadata, memory paths, and
delegated instructions are dynamic inputs and are not copied into this registry.

Every local model-facing tool adapter and its description lives in a flat
`src/tools/<tool>/` module. Domain state stays in its subsystem: for example,
the task adapters call `TaskManager`, and `load_skill` calls `SkillRegistry`.
Other stable model instructions live with the behavior that assembles them,
such as `src/artifact/model-instruction.md`.

These are compile-time assets, not runtime overrides or dynamically discovered
plugins. External executable tools integrate through MCP.

Tool descriptions are sent through the provider's sorted tool-schema
field. Core history schemas are present from the first normal call regardless
of the automatic compaction trigger. A frozen registry may include delegation,
web, or MCP capabilities selected during the single run-assembly path. Whether
a tool may be named by `spawn(kind=tool)` is explicit registration policy; the
exact allowed names appear in the `spawn` schema. Memory uses ordinary file
tools and therefore adds no schema. GeneralTask is assigned a delegating or
leaf variant from its remaining depth before it starts; every assembled profile
is then frozen for the run. Compaction reuses that system/tool prefix and adds
the `compaction_request` prompt as the final user message.
