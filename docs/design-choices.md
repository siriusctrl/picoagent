# Design Choices

Use this doc to record durable architecture choices that were actively considered and intentionally selected.

The goal is not to capture every idea. The goal is to avoid repeatedly reopening the same design discussions without new information.

Update this doc when:
- a boundary changes in a meaningful way
- a previously debated alternative is intentionally rejected
- a new service or bootstrap flow becomes part of the expected shape

Use this as the single source of truth for durable design decisions.

Other docs should:
- point here when they need to reference a stable architecture choice
- keep local implementation details and current behavior in place
- avoid re-explaining the same rejected alternatives unless the tradeoff changed

## Runtime Boundaries

The current top-level split is:

- `session` owns persistent context
- `runtime` reads control files, assembles prompts, decides when to compact, and executes runs
- `filesystem` provides file-backed inputs
- `execution backend` provides command execution

This split was chosen over a single mixed environment object.

Reason:
- it keeps context, files, and execution concerns explicit
- it allows files and execution to evolve independently
- it keeps transports thinner because HTTP does not need to own runtime composition details

Rejected direction:
- one `environment` or `resource` abstraction that mixes file IO, control loading, and command execution

## Session Ownership

Sessions are persistent context containers, not the runtime itself.

Current shape:
- a dedicated session service may be started separately
- the runtime may bind to that service over HTTP
- clients still create sessions through the runtime API
- the runtime forwards session creation into the bound session service instead of forcing clients to talk to both systems

Reason:
- session storage can be isolated from runtime execution
- clients still get one primary runtime surface
- the runtime remains the orchestration point without owning session persistence locally in all modes

Rejected direction:
- making clients create sessions directly against the session service as the main flow

## Filespaces And Namespace Paths

Tool-facing file access is modeled as mounted namespace paths, not `target + path` pairs.

Examples:
- `/workspace/src/http/server.ts`
- `/session/summary.md`
- `/remote@build/logs/output.txt`

Reason:
- one consistent path model is simpler for tools and models
- new mounts do not require changing every tool schema
- the runtime owns path parsing and mount resolution instead of pushing that burden onto the model

Rejected direction:
- preserving separate tool parameters like `target: workspace | session`

## Namespace Labels

A namespace label is just a runtime-facing mount name.

It may contain `@` to avoid ambiguity:
- `remote@build`
- `dataset@train`

`@` is a naming convention, not a protocol or parser feature.
The first path segment is always treated as the full namespace label.

Reason:
- labels only need to be unique and readable
- the model should not need to know backend IDs or real mount roots

Rejected direction:
- adding special URI syntax or backend-aware parsing rules for namespace labels

## Namespace Capabilities And `cmd`

Namespace mounts carry simple capabilities instead of baking runtime policy into the mount label itself.

Current shape:
- `supportsCmd` declares whether `cmd` is allowed for that namespace
- the mount name stays a readable namespace label, not the source of truth for command policy
- `cmd` requires an explicit namespace-rooted `cwd` instead of silently choosing one

Reason:
- `cmd` policy stays explicit and tied to the current tool surface
- a writable namespace and a `cmd`-enabled namespace are related but not forced to be the same thing
- the model should not rely on hidden runtime defaults for where commands execute

Rejected direction:
- overloading the namespace label itself to mean both identity and command policy
- silently defaulting `cmd` to some preferred namespace when multiple mounted surfaces exist

## Control Inputs

Control inputs come from workspace and host files, but they are not exposed as a separate mounted `/control` filesystem and they are not cached inside session state.

Current shape:
- control files live in the workspace
- host defaults are also read during control loading
- the runtime keeps a workspace-scoped control snapshot cache
- before each run it checks the relevant control files and rebuilds only when the control version changes

Reason:
- there should not be two visible truths for the same control files
- control is primarily runtime prompt input, not a tool-facing writable surface
- agents can still edit control files through the workspace when needed
- session storage stays pure context instead of becoming a second prompt-policy container

Rejected direction:
- exposing a separate `/control` mount alongside `/workspace`
- storing cached control snapshots on the session as the source of run behavior

## Session History Surface

Session history is currently exposed as a dedicated read-only projection rather than a general mounted writable filesystem.

Current shape:
- the model can inspect `/session/...`
- the projection is read-only
- raw event history remains available through HTTP session resources and run event endpoints

Reason:
- session browsing is useful for model-side inspection
- keeping it read-only avoids conflating context state with ordinary workspace editing

Open issue:
- this may later become a more general mounted filesystem surface if there is a strong concrete need

## Session Compaction Model

Session compaction uses `checkpoint + tail`, not history deletion.

Reason:
- older context can be compressed without pretending it never existed
- live interaction keeps a recent tail instead of replacing the whole session with a summary
- session browsing and HTTP resource reads can continue to expose the longer history

Rejected direction:
- destructive history rewriting as the primary compaction model

## One Runtime Tool Surface

The runtime exposes one general tool surface instead of multiple built-in agent presets.

Current shape:
- the registry contains one stable set of tools
- control files shape prompting, not which preset name is active
- sessions do not carry a default agent field

Reason:
- the harness stays smaller and easier to reason about
- runtime policy has one source of truth
- session remains a context store instead of a policy container

Rejected direction:
- carrying built-in `ask` and `exec` presets through API, session state, and prompt assembly

## Runtime Surface

Clients should primarily talk to the runtime, even when session or filespace services are running separately.

Current shape:
- the runtime is the main client-facing surface
- filespace and session services can be started separately and then bound into the runtime
- clients are not expected to coordinate directly with multiple backends in the normal flow

Reason:
- one main product surface is simpler for clients
- isolation can still exist behind the runtime boundary
- orchestration stays in the runtime instead of being pushed onto every client

Rejected direction:
- making clients coordinate directly with session and filespace services as the primary product flow

## Tool Surface Across Filesystems

Different filesystem implementations should support the same core tool surface where the semantics match.

Current expectation:
- `read`
- `glob`
- `grep`
- `patch`

Reason:
- the model should see one stable capability surface
- local disk, shell-backed, and in-memory filesystems can differ internally without fragmenting the tools

Rejected direction:
- per-backend tool names such as separate tools for local, remote, or in-memory filesystems

## OpenAPI Source Of Truth

OpenAPI is generated from Hono route definitions and Zod schemas.

Current shape:
- schemas live in `src/http/openapi-schemas.ts`
- routes live in `src/http/openapi-routes.ts`
- the document is built in `src/http/openapi-document.ts`
- the exposed spec path is `/openapi`

Reason:
- route contract and schema stay co-located
- the spec is not maintained as a second hand-written truth

Rejected direction:
- manually maintaining a separate hard-coded OpenAPI document

## Toolchain

The repository is Bun-first for local development, test execution, and CI.

Current shape:
- dependency installation uses `bun install`
- package scripts are expected to run through `bun run`
- tests run through Bun's native test runner
- source entrypoints run directly from `.ts` and `.tsx` files
- source imports use `.ts` and `.tsx` extensions, with `tsc` rewriting them for emitted JavaScript
- `tsc` is still used for `build` and `typecheck`
- local runtime code should prefer Bun-native APIs over Node compatibility shims when Bun already provides the needed primitive
- Bun-specific primitives still need to stay behind the existing HTTP, filesystem, and execution boundaries

Reason:
- Bun gives a simpler TypeScript and TSX runtime story for this project
- it removes the extra `tsx` loader layer from normal development and test flows
- it improves local iteration and keeps the default runtime stack smaller
- it still keeps the domain model transport-agnostic because Bun usage is confined to local backend implementations

Rejected direction:
- keeping npm plus `tsx` as the default workflow for local development and CI

## Source-Area Guidance

Repository guidance docs belong under `docs/`, not under `src/`.

Current shape:
- high-level rules live in `AGENTS.md`
- source-area guidance lives in `docs/source-map.md`
- durable architecture decisions live in this file

Reason:
- keep source directories free of scattered local instruction files
- make navigation and design history easy to find from the repo root
