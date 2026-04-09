# Source Map

Use this doc for source-area guidance instead of scattering local `AGENTS.md` files through `src/`.

## `src/core`

Scope:
- agent loop
- provider contract
- tool registry
- shared message and tool types

Rules:
- keep this directory transport-agnostic
- keep HTTP, Ink, and provider SDK details out
- validate external and model-produced inputs before core logic assumes shape
- if outward contracts change here, update deterministic tests

Read first:
- `docs/architecture.md`
- `docs/runtime-model.md`
- `src/core/loop.ts`
- `src/core/types.ts`
- `src/core/tool-registry.ts`

## `src/runtime`

Scope:
- assemble the runtime context
- define the global tool registry
- build session control snapshots from workspace control files

Rules:
- keep business logic out of runtime context assembly
- prefer changing registry assembly over adding agent-specific special cases elsewhere
- keep session control snapshot behavior explicit and file-driven

Read first:
- `docs/architecture.md`
- `docs/runtime-model.md`
- `src/runtime/index.ts`
- `src/runtime/control-snapshot.ts`

## `src/http`

Scope:
- minimal async HTTP adapter over the shared runtime

Rules:
- keep the endpoint surface narrow and explicit
- keep the resource model explicit: sessions, runs, and events
- prefer direct request handlers over framework-looking abstractions
- reuse the same runtime context and loop semantics as other transports
- add deterministic tests for endpoint and event contract changes

Read first:
- `docs/entrypoints.md`
- `docs/architecture.md`
- `src/http/server.ts`

## `src/tools`

Scope:
- LLM-facing capabilities

Rules:
- keep tools focused: one tool, one capability
- validate parameters with schema checks at the boundary
- prefer shared helpers for path resolution and filesystem behavior
- do not scatter agent gating into tool implementations when the registry boundary is enough
- update deterministic tests when a tool contract changes

Read first:
- `docs/runtime-model.md`
- `src/core/tool-registry.ts`
- the specific tool file you are changing

## `src/clients/cli`

Scope:
- minimum command-line service surface

Rules:
- keep the CLI thin and explicit
- reuse the HTTP surface instead of inventing a second runtime path
- prefer a small command surface over feature-heavy shell UX
- if CLI behavior changes, verify with `npm run dev:cli -- help` and `serve`

Read first:
- `docs/entrypoints.md`
- `src/clients/cli/main.ts`
- `src/http/server.ts`

## `src/clients/tui`

Scope:
- local terminal debug client for the HTTP server

Rules:
- keep the TUI thin; it should speak HTTP and render terminal UX
- do not move model logic here
- preserve terminal-native behavior over browser-style patterns
- if TUI behavior changes, verify with `npm run dev:tui`

Read first:
- `docs/entrypoints.md`
- `src/clients/tui/controller.ts`
- `src/clients/tui/main.tsx`

## `src/config`

Scope:
- configuration loading
- provider environment resolution

Rules:
- keep config parsing deterministic and narrowly scoped
- avoid pulling HTTP, client, or SDK concerns in here

Read first:
- `docs/architecture.md`
- `src/config/config.ts`

## `src/fs`

Scope:
- deterministic filesystem helpers
- workspace filesystem boundary

Rules:
- preserve session-root safety and predictable traversal behavior
- keep helpers deterministic and free of HTTP or UI concerns

Read first:
- `docs/architecture.md`
- `src/fs/filesystem.ts`
- `src/fs/workspace-fs.ts`

## `src/prompting`

Scope:
- prompt assembly
- frontmatter-backed prompt scanning

Rules:
- keep prompt helpers deterministic and file-driven
- avoid mixing provider, HTTP, or client concerns into prompt construction

Read first:
- `docs/runtime-model.md`
- `src/prompting/frontmatter.ts`
- `src/prompting/prompt.ts`

## `src/providers`

Scope:
- provider SDK adapters only

Rules:
- keep SDK imports here, never in `src/core`
- translate between provider payloads and core message/tool shapes
- validate provider responses at the boundary before converting them into core types

Read first:
- `src/core/provider.ts`
- the provider adapter you are changing
