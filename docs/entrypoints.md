# Entrypoints

Fiasco exposes its multi-agent runtime through one binary, `fiasco`, and a
reusable library.

## Commands

- `fiasco run`: execute a task and persist its run directory.
- `fiasco inspect`: open a tail-first snapshot of committed transcript records
  on a TTY; redirected stdout writes exact committed NDJSON.
- `fiasco inspect --follow`: refresh a running transcript in the interactive
  viewer. It requires a TTY.
- `fiasco inspect --output ndjson`: explicitly select newline-complete NDJSON.
- `fiasco inspect --summary`: print run metadata and final output.
- `fiasco auth login`: OpenAI OAuth device login.
- `fiasco memory consolidate`: run model-driven user/project memory maintenance.
- `fiasco skills list`: inspect discovered skill metadata.

`fiasco run --output ndjson` emits transport-neutral runtime events. A future HTTP
or queue worker should call the library and forward the same events. It should
not create another model/tool loop.

Inspect is dispatched after workspace resolution but before application config,
provider, MCP, skill, hook, or model initialization. Reading a portable run
therefore does not require usable provider credentials or configuration. The
interactive path embeds fmtview through `fmtview::view` and delegates physical
newline paging and follow refresh to fmtview-core's generic file timeline.
Fiasco adds only run routing, terminal-state mapping, and command selection.
