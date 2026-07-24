# ADR 0038: Use Runtime Handles and Explicit Restart

- Status: Accepted
- Date: 2026-07-23
- Supersedes: ADR 0027 (durable delegate-recovery correlation)
- Supersedes: ADR 0035 (delegated agents as persistent tasks)
- Supersedes: ADR 0036 (reconstructed interruption state)
- Refines: ADR 0034 (checkpoint recovery scope)

ADR 0041 refines `close` so it may terminate an active agent activity before
persisting the thread's closed lifetime.

ADR 0044 later replaces complete-checkpoint loading with newline-visible
messages and minimal trailing tool-turn repair. The explicit crash notice and
non-reconstruction policy here remain accepted.

The current implementation also makes child input entirely process-local until
it is appended to `messages.jsonl`. This removes the pending-input log and its
message-local idempotency metadata without changing the explicit-restart policy.

ADR 0047 later collapses durable run state to `open | completed | closed` and
folds selected-handle inspection into `list_handles`. Activity failures remain
events, and the separate `status` tool is removed without a compatibility
alias.

## Context

Delegated agents and long-running ordinary tools need the same small control
surface while the process is alive. They do not need the same durable object.
An agent has a useful identity, transcript, parent, and open lifetime after an
activity ends. An ordinary tool future does not.

Persisting both as tasks created a second authority beside each child run. Crash
recovery then reconciled task records, child states, pending input, activity
outputs, delivery cursors, and originating calls. That complexity attempted to
make a rare process failure look transparent even though external side effects
could not be resumed exactly once.

## Decision

- Expose one runtime-handle control surface: `delegate`, `list_handles`,
  `status`, `wait`, `stop`, `send_message`, `inspect`, and `close`.
- Use the child run id as a delegated agent's handle. Persist its non-empty
  model-supplied display name in `run.json`; the name is opaque metadata and
  does not determine identity or paths.
- Give an ordinary tool future that outlives the shared foreground window a
  process-local `j_<ulid>` handle. Preserve the exact future without stopping
  or restarting it.
- Keep activity state, queued followups, pending output, generations, and tool
  handles in memory. Do not add a persistent handle index or task record.
  `list_handles` discovers direct child runs by `parent_run_id` and overlays
  current-process execution state.
- Keep `steer` and `followup` as the required `send_message` modes. A completed
  or stopped activity leaves its agent thread idle; only `close` ends the
  durable thread.
- Make `wait` wait for any selected handle to produce a result or change state.
  An empty handle list means all visible handles.
- On root restart, repair the message tail and unconditionally tell the model
  that the prior process, its mailbox input, and all of its
  asynchronous activity stopped. Do not reconstruct, relaunch, or synthesize
  terminal results for old tool jobs or child activities.
- Keep old open child threads discoverable but inert. The first explicit
  `send_message` adds a child crash reminder and starts a new activity from its
  complete transcript.
- Keep the parent run as the only public resume entrypoint.

## Consequences

- Agent identity and useful context survive activity completion and process
  failure without a second durable scheduler object.
- Ordinary asynchronous tool work is intentionally lost with the process.
- A parent may need to inspect workspace or external state and explicitly retry
  work after a crash. The runtime states this uncertainty instead of claiming
  transparent continuation.
- There is no recovery reconciliation across parent task files, child state,
  output cursors, or pending-input queues.
- Runtime notices correlate later results by handle. Provider tool-call ids
  remain the foreground result correlation only.
- Existing pre-release task records and task-control tool names are not a
  compatibility target.

## Alternatives Considered

- **Keep one durable task abstraction for agents and tools.** Rejected because
  the shared control surface does not require shared persistence, and tool
  futures have no useful post-process identity.
- **Automatically resume child activities.** Rejected because it expands crash
  reconciliation without providing exactly-once external effects.
- **Persist interruption outputs and queued followups.** Rejected because a
  single crash notice plus model-directed inspection and retry is sufficient
  for the expected rare failure.
- **Add a separate agent registry.** Rejected because child run records already
  contain durable identity, parent ownership, display name, and transcript.

## Related Documents

- [ADR 0034: Resume from atomic turn checkpoints](0034-atomic-turn-checkpoints.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
