# Runtime Model

The runtime model is intentionally small:

- `session` stores context
- `runtime` reads control files, assembles prompts, and executes runs
- `filesystem` provides file-backed surfaces
- `execution backend` runs commands
- `run` is one execution through the runtime

Use `docs/design-choices.md` for the durable reasoning behind these boundaries.

## Workspace And Control Files

The current workspace is the main writable file surface.

It can contain ordinary project files plus control files such as:

- `.pico/config.jsonc`
- `AGENTS.md`
- `SOUL.md`
- `USER.md`
- `.pico/memory/`
- `skills/`

Host defaults such as `$HOME/.pico/config.jsonc` and `$HOME/.pico/memory/` are also read during control loading.

The runtime keeps a control snapshot cache per workspace.
Before a run starts it checks the relevant control files, reuses the cached snapshot when nothing changed, and rebuilds it only when the control version changes.
There are no built-in runtime agent presets anymore.

## Session

Each conversation is one session.

In the current local harness, a session is created against one workspace root.
That workspace binding is local-host behavior, not the main point of the abstraction.

A session stores:

- workspace root and related roots
- conversation history
- ordered run ids
- active-run state
- optional checkpoints created by compaction

The session does not carry prompt-assembly policy.
It is append-only context storage plus the metadata needed to project that history back out.

When the runtime is bound to an external session service:

- clients still create sessions through the runtime API
- the runtime forwards session creation into the external session service
- the runtime reads session state back through the session-store boundary
- run-facing session projections such as `/session/...` stay consistent

## Run Execution

For each run, the runtime:

1. checks the current control version
2. reuses or rebuilds the cached control snapshot
3. creates the provider
4. executes the shared agent loop with the general tool surface

That means workspace control changes apply on the next run without mutating stored session state.

## Session History

Session compaction uses `checkpoint + tail`, not destructive history rewriting.

After compaction:

- older turns are summarized into a checkpoint
- recent messages stay live
- runs and run events are still available to clients

For model-side inspection, the session exposes a read-only file-view:

- `summary.md`
- `checkpoints/<id>.md`
- `runs/<id>.md`

This projection is for browsing context, not for editing the session itself.
Raw run events remain available through HTTP event and session-resource endpoints.

## File Views And Tools

The runtime exposes one general tool surface:

- `glob`
- `grep`
- `read`
- `patch`
- `cmd`

These tools operate on file-view paths such as:

- `/workspace/src/http/server.ts`
- `/session/summary.md`

Mounted file-views may declare `supportsCmd`.
`cmd` always requires an explicit namespace-rooted `cwd`.

`grep` prefers `rg` on a cmd-enabled workspace when available and falls back to the built-in file-view search otherwise.
`glob` follows Bun glob semantics in the default local runtime.

## Events

Run events are append-only records for one execution.

They back:

- run snapshots
- `GET /events/:runId` JSON reads
- `GET /events/:runId` SSE streams

Session-wide streaming is still missing.
