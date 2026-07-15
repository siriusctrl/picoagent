# Prompt Assets

Stable model-facing prose lives in Markdown and is embedded into the binary
with Rust `include_str!`. Runtime assembly, precedence, dynamic values, tool
schemas, and execution contracts remain in Rust.

- `agents/`: stable agent-level instructions.
- Tool descriptions live beside their owning implementations. Standalone base
  tools use `src/tools/<tool>/description.md`; subsystem tools remain with their
  subsystem.

These files are compile-time assets, not runtime overrides or dynamically
discovered plugins. Project-specific instructions belong in `AGENTS.md`, and
external executable tools integrate through MCP.
