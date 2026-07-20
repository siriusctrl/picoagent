# ADR 0020: Unify Background Task Runtime Notices

- Status: Accepted
- Date: 2026-07-20
- Refines: ADR 0017 (background acknowledgement and delivery protocol)
- Refines: ADR 0018 (terminal background result presentation)

## Context

Delegated children and automatically promoted tools shared durable task state,
but exposed different model-facing shapes. Start acknowledgements were JSON,
terminal records used a separate background-result tag, and small terminal
values could be embedded while large values used the artifact envelope. A
promoted result also exposed its internal originating call id. These variants
made prompt guidance, resume reconstruction, message storage, and model behavior
harder to understand.

Several ready task results also produced consecutive user messages. Although
supported by the provider contracts, one bounded runtime message is simpler for
compatible endpoints and avoids repeating control framing.

## Decision

- Use `<background_task>` for both active and terminal task notices, always
  inside `<runtime-reminder>`.
- A status-less block is an acknowledgement that work is running. Its body is a
  short runtime sentence, not a result.
- A terminal block includes `status="completed|failed|cancelled|interrupted"`.
  Its body is only the workspace-relative path to the complete result artifact.
- Persist every terminal result as an artifact regardless of size, including
  errors and cancellation or interruption details. Do not put previews or
  original output in the terminal notice.
- Batch all task records ready at one model boundary into one user message with
  one outer runtime reminder and one block per task.
- Require `delegate` to accept a short model-supplied `name` as well as its
  prompt. Promoted tools use the tool name. Task controls expose task id, name,
  and status but not the internal task kind.
- Keep task kind and a promoted tool's originating provider call id in durable
  internal state for recovery and control. Do not expose them in runtime XML.
- Resume reconstructs a missing original tool result with the same status-less
  acknowledgement even when the durable task has since finished.
- Artifact-backed history matches return the exact artifact path in addition to
  their owning message ref and snippet.

## Consequences

- The provider still receives exactly one tool result for each original tool
  call. Terminal delivery remains a later user/runtime message correlated by
  task id.
- Small background results incur one artifact file and sidecar, but parent
  context size and parsing no longer depend on result size.
- The model must explicitly read an artifact before relying on terminal output.
  Stable prompt guidance documents that requirement.
- One message can contain several result paths. Returning the matching artifact
  path from history search removes ambiguity when only one artifact matched.
- Task display names improve inspection but are labels, not identifiers; task
  controls continue to use the short run-local task id.

## Alternatives Considered

- **Keep JSON starts and a separate terminal tag.** Rejected because two
  protocols describe one lifecycle and require duplicate parsing and guidance.
- **Inline small terminal results.** Rejected because terminal message size and
  shape would still vary, complicating batching and model behavior.
- **Return a bounded preview in each terminal block.** Rejected because several
  simultaneous completions can still crowd one runtime message; the artifact
  is already readable with ordinary tools.
- **Expose task kind and originating call id.** Rejected because they are
  recovery details, not information needed for model control.
- **Append one user message per terminal task.** Rejected because a single
  boundary naturally represents one ready set and repeats less framing.

## Related Documents

- [ADR 0017: Concurrent tool batches and explicit task controls](0017-concurrent-tool-batches-and-explicit-task-controls.md)
- [ADR 0018: Limit tool output per result](0018-limit-tool-output-per-result.md)
- [Runtime model](../runtime-model.md)
- [Artifact contract](../artifacts.md)
- [Architecture](../architecture.md)
