---
name: register-mcp
description: Convert an MCP stdio server into a validated Fiasco MCP artifact with a model-generated namespace, concise catalog description, capability-oriented source map, exact captured tool catalog, and tested CLI-like calls. Use when adding, refreshing, documenting, or repairing an MCP integration for Fiasco.
---

# Register MCP

Turn a raw MCP server into progressive documentation plus an exact executable
catalog. Generate the knowledge layer yourself; use Fiasco's MCP commands for
capture, loading, compilation, and calls so validation exercises the same Rust
implementation as the runtime.

Read [references/artifact-format.md](references/artifact-format.md) before
writing an artifact.

## Build the Artifact

1. Inspect the proposed command, arguments, environment-variable names, and
   available server documentation. Never copy credential values into the
   workspace.
2. Add a provisional `[mcp.<name>]` entry to `.fiasco/config.toml`, including
   its future `artifact` directory. The active Fiasco process does not reload
   this edit.
3. Run `fiasco --workspace <workspace> mcp capture <name>`. This connects to the
   server and writes its exact `tools/list` result to `catalog.json`.
4. Inspect the captured tool names, descriptions, schemas, server package, and
   any available upstream documentation. Generate a concise lowercase
   namespace and description that represent the whole capability.
5. Rename the provisional config key and artifact directory to the chosen
   namespace. Make the config key, directory intent, and `MCP.md` name agree.
6. Write `MCP.md` as a short capability source map. Write detailed Markdown
   under `references/` only where it helps the using model choose and combine
   commands.
7. Run the offline and live checks. Compile representative calls for every
   documented capability. When safe and possible, execute representative
   read-only calls and refine the documentation from their real behavior.

Treat `catalog.json` as captured machine data. Regenerate it through `capture`;
do not manually rewrite remote names or schemas.

## Organize for Progressive Use

Organize the source map around user goals and workflows, not the raw
`tools/list` order. Aggregate commands when they operate on the same objects,
share identifiers or constraints, or are commonly used together. For example,
put code search, file reads, commit listing, and commit inspection together
when they form one repository-research workflow.

Keep unrelated commands separate even when they come from the same server.
Avoid both one document per trivial command and one giant document for the
whole server. This is an authoring judgment, not a validator rule.

In each detailed reference:

- state the user-facing capability and when to choose it;
- list exact remote tool names and compact CLI signatures;
- show commands using Fiasco's one supported syntax;
- explain shared identifiers, sequencing, pagination, output shapes, and
  observed constraints;
- distinguish read-only operations from actions with external effects.

## Explore Before Finalizing

Explore the server when credentials, useful fixtures, and safe operations are
available. Prefer list, search, get, and inspect commands. Exercise different
parameter types, empty results, pagination, and ordinary errors when they
clarify the contract.

Do not create, modify, delete, publish, pay, or send external messages merely
to improve documentation. Run those calls only when the user's task already
authorizes the effect. If live exploration is unavailable, finish the
artifact from captured schemas and source documentation, then report exactly
which capabilities were not exercised.

## Verify

Run:

```bash
fiasco --workspace <workspace> mcp check <name>
fiasco --workspace <workspace> mcp check <name> --live
fiasco --workspace <workspace> mcp compile "<name> <tool> key=value"
```

Use `scripts/verify-artifact.sh` to combine artifact loading, optional live
catalog comparison, and any number of representative compile checks:

```bash
scripts/verify-artifact.sh <workspace> <name> \
  "<name> <tool> key=value" \
  "<name> <other-tool> 'one positional value'"
```

For safe representative calls, run:

```bash
fiasco --workspace <workspace> mcp call "<name> <tool> key=value"
```

An artifact is complete only when it loads through `mcp check`, its documented
examples compile to the intended remote tool and arguments, its live catalog
matches when the server is reachable, and the source map lets a model find
the relevant reference without loading the entire catalog.
