# ADR 0047: Collapse Durable Run Lifetime and Handle Discovery

- Status: Accepted
- Date: 2026-07-24
- Refines: ADR 0024 (persisted agent profiles)
- Refines: ADR 0038 (durable lifetime and handle controls)
- Refines: ADR 0045 (terminal transcript boundary)

## Context

`run.json` stored queued, running, open, completed, failed, cancelled, and
closed states even though execution activity already had process-local handle
state, lifecycle events, and an exclusive filesystem lease. A failed root could
be resumed, so `failed` was not a durable lifetime boundary. `cancelled` had no
production writer.

GeneralTask delegating and leaf profiles selected the same model role and
frozen tool schemas. Their only difference was a redundant consistency check
against `remaining_delegation_depth`, which already controls whether a child
can delegate.

The model-facing `list_handles` and `status` tools returned the same snapshot
shape. One discovered all visible handles and the other selected named handles,
forcing every run to carry two schemas for one read-only operation.

## Decision

- Persist only `open`, `completed`, and `closed` run lifetime. New roots and
  children start open. Root success marks completed; explicit child closure
  marks closed.
- Use the execution lease and process-local handle state to represent active
  work. Record activity and root failures as events and leave the durable run
  open for model- or user-directed retry.
- Allow public resume only for an open root. Completed and closed runs reject
  resume; child ids remain outside the public resume path.
- Persist only `root` and `general_task` profiles. Keep exact
  `remaining_delegation_depth` as the sole durable delegation authority.
- Remove persisted absolute depth, parent depth propagation, and copied
  GeneralTask instructions. GeneralTask guidance comes from the current typed
  prompt during initial request assembly, just as the shared system prompt
  does.
- Retain the per-run tool-schema hash, provider resume fingerprint, model,
  modality declaration, prompt, parentage, and display name.
- Keep only `list_handles` for read-only handle discovery and snapshots.
  Omitted or empty `handles` discovers all visible handles; named handles return
  their current snapshots; `include_closed` extends all-handle discovery.
  Remove `status` and its schema without an alias.
- Advance the pre-release run record to version 15. Older records are not
  migrated.

## Consequences

- `run.json` no longer duplicates process-local execution status or failure
  history. Operators use the lease and events for those concerns.
- A failed activity remains explicitly retryable because its run stays open.
  A process crash does not require durable activity-state reconciliation.
- Root and GeneralTask remain distinct model roles, while remaining delegation
  capacity alone decides whether `delegate` succeeds.
- Every normal request carries one fewer built-in tool schema, and the model
  uses one operation for both handle discovery and named status checks.
- GeneralTask instruction edits apply to a child that has not yet written its
  first message. Fiasco does not claim stronger prompt freezing than it provides
  for the shared system prompt.

## Alternatives Considered

- **Persist running and failed for observability.** Rejected because the lease
  and lifecycle events already own those facts, while failed is not terminal.
- **Keep delegating and leaf profiles as a consistency check.** Rejected because
  the remaining-depth field is the actual authority and configuration changes
  do not override it.
- **Keep absolute depth for hook payloads.** Rejected because observability-only
  metadata does not justify another durable delegation field.
- **Keep `status` as an alias.** Rejected because the harness has no external
  compatibility promise and an alias defeats schema ablation and simplification.

## Related Documents

- [Runtime model](../runtime-model.md)
- [Architecture](../architecture.md)
- [Design choices](../design-choices.md)
- [ADR 0024](0024-freeze-built-in-schemas-across-agent-roles.md)
- [ADR 0038](0038-runtime-handles-and-explicit-restart.md)
- [ADR 0045](0045-delegate-transcript-paging-to-fmtview-core.md)
