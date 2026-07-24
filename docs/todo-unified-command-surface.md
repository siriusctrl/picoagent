# TODO: Unified Command Surface

## Handoff Status

- Status: design handoff; implementation has not started.
- Baseline: `a269e98d85878508494f96ceed9bd038534fe236`
  (`feat(mcp): add progressive MCP artifacts`).
- Target repository: `/cpfs03/user/niuxinyao.nxy/fiasco`.
- Intended owner: the next implementer should validate the proposal against the
  current checkout before editing because this document describes a target, not
  an accepted runtime contract.

This task explores reducing the provider-visible built-in surface through two
candidate designs:

1. one stable in-process command tool that routes most capabilities; or
2. a Bash-first surface where `read`, file mutation, and `bash` remain fast
   native tools, stateless capabilities become real Fiasco CLI commands invoked
   through Bash, and only process-local runtime control retains a small native
   adapter.

The handoff must test the smaller Bash-first design before committing to a
general command registry. Both designs keep domain execution in focused Rust
handlers.

Do not turn this proposal into an ADR until the command surface has been tested
on real trajectories and the maintainer accepts the final contract.

## Candidate A: One In-Process Command Tool

The original proposal gives the provider one fixed tool, provisionally named
`fiasco`:

```yaml
name: fiasco
description: >-
  Invoke one Fiasco command. Use one call per independent operation; multiple
  calls in one assistant response execute concurrently. Put scalar options in
  command and exact multiline payloads in stdin.
input_schema:
  type: object
  properties:
    command:
      type: string
      minLength: 1
    stdin:
      type: string
      description: Exact optional UTF-8 standard input for this command
  required: [command]
  additionalProperties: false
```

`fiasco` is a working name. `invoke` or `command` are credible alternatives;
confirm the final provider-visible name with the maintainer before the cutover.
Do not carry aliases after the experiment.

Internally, file operations, shell execution, agent coordination, history,
skills, web search, and MCP remain separate Rust handlers. Only their
provider-facing registration and argument compilation are unified.

```text
provider tool call                 fiasco invoke CLI
{command, stdin?}                  argv plus process stdin
          \                         /
           +-- shared compiler ----+
                       |
        CompiledInvocation { route, arguments }
                       |
              deterministic registry
          /        /       |       \       \
       read     patch   history   agent    mcp
                       |
          existing ToolContext and RawToolOutput
                       |
       hooks, events, artifacts, foreground window,
       background promotion, ordered result delivery
```

The command router is not a shell. It must not execute command strings through
`bash`, support pipelines, or interpret command separators. One provider tool
call represents exactly one Fiasco operation.

This remains a candidate, not the implementation default. A fixed tool of this
size is already cheap; removing it is useful only if the replacement also makes
the runtime simpler.

## Decisions Already Made In Discussion

- Collapse the provider-visible tool surface, not the internal handler
  boundaries.
- Reuse one execution path for every surface that invokes the same capability.
  Candidate A shares a command compiler; Candidate B shares real CLI handlers.
- Keep model-visible routes flat by default. Nest only to resolve an actual
  ambiguity or represent a dynamic namespace. Code-directory ownership does
  not dictate command paths.
- Preserve one call per independent operation. Do not add a multi-command
  script or pipeline syntax; the existing assistant tool-call batch already
  provides concurrency.
- Carry exact multiline or otherwise awkward payloads through stdin. Candidate
  A uses the outer tool's optional `stdin` string; Candidate B uses real process
  stdin.
- Treat shell heredoc syntax as a CLI frontend feature. For example:

  ```bash
  fiasco invoke "patch" <<'PATCH'
  *** Begin Patch
  *** Update File: src/main.rs
  @@
  -old
  +new
  *** End Patch
  PATCH
  ```

  The user's shell converts the heredoc into process stdin. The shared Fiasco
  compiler receives `command = "patch"` plus the exact stdin bytes. Do not
  implement `<<`, delimiter quoting, `<<-`, or any other heredoc grammar in
  Rust.
- Make `patch` the normal existing-file mutation command. Keep an explicit
  `write <path>` command for new files and intentional complete
  replacements; its complete UTF-8 content comes from stdin.
- Reuse a familiar patch representation rather than inventing a new
  search/replace DSL. The initial scope should preserve the current
  deterministic, conservative matching behavior.
- Keep `web search`; `web` alone is not a sufficiently clear action.
- Do not add a generic `command | command` pipeline. Independent operations use
  sibling provider calls. A future command may declare domain-specific
  variadic inputs when there is a real need, but that is not pipeline
  execution.
- Under Candidate A, MCP becomes the `mcp` command namespace instead of
  retaining its own provider-visible tool. Under Candidate B, MCP may use its
  real CLI only if reconnecting per call preserves the required behavior.
- Keep command discovery progressive and deterministic. Removing a command for
  an ablation must also remove its model guidance.
- Do not add compatibility layers for unreleased runs or hypothetical external
  users. A temporary dual surface is acceptable only inside an explicit
  experiment and must be deleted at cutover.

## Current Baseline

Read these files before changing the design:

- `src/tools/mod.rs`: current `Tool`, `ToolRegistry`, `ToolContext`, and
  `RawToolOutput` contracts.
- `src/tools/assembly.rs`: process-wide and run-scoped tool registration.
- `src/agent/tool_execution.rs`: concurrent batches, hooks, events, artifact
  persistence, foreground timeout, and exact-future promotion.
- `src/agent/runner.rs`: tool-spec freezing and `tool_schema_sha256`.
- `src/model/mod.rs`: provider-neutral `ToolSpec`, `ToolCall`, and exact raw
  argument persistence.
- `src/mcp.rs` and `src/mcp/command.rs`: the first fixed command-tool prototype
  and schema-aware command compiler.
- `src/tools/write/mod.rs`: complete writes, file locks, BOM/line-ending
  preservation, conservative replacement matching, and atomic replacement.
- `src/tools/*/tool.yaml`: current capability-local descriptions and schemas.
- `tests/direct_tool_batch.rs`: concurrency, ordering, promotion, and
  attachments.
- `tests/mcp_cli.rs`: one shared MCP artifact/compiler/call path.

At the baseline there are fourteen built-in `tool.yaml` files plus the fixed
MCP manifest. Their raw source is 13,767 bytes before provider JSON wrapping.
The point of this task is not merely to delete those bytes from the repository;
it is to stop sending every detailed capability schema on every normal model
request.

The current runtime already permits several calls with the same tool name in
one assistant response. Calls have distinct ids, start concurrently, and commit
results in original call order. Candidate A's single provider-visible `fiasco`
name must not change that behavior.

## Candidate B: Bash-First Real CLI

The more aggressive alternative removes the general provider-visible
`fiasco({command, stdin})` adapter. Keep a very small native surface:

```text
read        bounded text and native image reads
write       explicit complete file content
patch       conservative multiline file edits
bash        ordinary host commands and stateless Fiasco CLI entrypoints
runtime     only capabilities that require the current process, if still needed
```

The `runtime` name is provisional. The important distinction is process
ownership, not naming.

Stateless capabilities can become real CLI commands and run through Bash:

```bash
fiasco history search 'McpRuntime'
fiasco history read m42
fiasco skill load register-mcp
fiasco web search 'query one'
fiasco mcp call "github search_code query='CommandRegistry'"
```

This path genuinely reuses the user's shell for argv, quoting, redirection, and
heredoc handling. Clap or a focused subcommand parser owns the Fiasco CLI shape.
Do not add a second shell-like parser behind `bash`.

### Process-Local Boundary

A Bash call starts a child shell. A `fiasco` command launched by that shell is
another Fiasco process and cannot borrow the parent process's Rust state:

- `RuntimeHandleManager`;
- active agent activities;
- steer/followup mailboxes;
- promoted ordinary-tool futures such as `j_<ulid>`;
- pending process-local result delivery;
- already connected MCP clients and server sessions.

Consequently, a child CLI cannot correctly implement:

```bash
fiasco wait j_01ABC
fiasco send 01CHILD --mode steer
fiasco stop j_01ABC
```

against the live parent merely by reading run files. Those states are
intentionally process-local.

Making these commands work from Bash would require one of:

- a Unix-socket or other RPC control server in the parent;
- inherited control file descriptors plus a CLI shim;
- magical interception of command strings beginning with `fiasco` inside
  `BashTool`.

Do not add any of these merely to remove one small provider schema. They create
a second execution/control subsystem, and prefix interception does not actually
reuse Bash parsing.

MCP is a separate decision. A stateless `fiasco mcp call` can reconnect for
every invocation, but that adds startup latency and may change the behavior of
sessionful servers. Keep the in-process MCP adapter until real tests show that
cold per-call CLI execution is acceptable.

### Recommended First Experiment

Candidate B is the smaller spike and should run before building Candidate A:

1. Preserve the current native `read`, file mutation, and `bash` tools.
2. Add real CLI entrypoints for a small stateless slice: `web search`,
   `history search/read`, and `skill load`.
3. Exercise those commands exclusively through `bash` with `qwen3.7-max`.
4. Measure command success, quoting corrections, process startup cost, token
   usage, and transcript quality.
5. Leave agent/handle controls and persistent MCP execution in process during
   the experiment.
6. Decide whether the remaining process-local controls justify one small
   `runtime`/`fiasco` adapter or whether the original native controls remain
   clearer.

The simplest system is not necessarily the one with the fewest provider tool
names. Prefer the option with fewer execution authorities and no hidden IPC.

## Candidate A Command Grammar

### Tokenization

Start from the existing MCP tokenizer, but make positionals explicit in each
command spec:

```text
<route...> [positional ...] [name=value ...]
```

- Tokenize `command` with `shell_words`.
- Use shell quoting only to group command-line tokens.
- Preserve schema-declared strings as strings.
- Convert top-level booleans, integers, numbers, arrays, objects, and nulls
  using the command's declared argument schema.
- Arrays and objects use JSON values inside one shell token.
- Reject duplicate named arguments.
- Reject unknown arguments when the handler does not allow them.
- Check required arguments before execution.
- Leave enums, ranges, path rules, and domain invariants to the typed handler
  unless a check is already a genuine compiler boundary.
- Do not build a complete JSON Schema engine.
- Do not support `;`, `&&`, `||`, pipes, redirections, environment assignment,
  glob expansion, variable expansion, command substitution, or subshells.

Each command declares its positional fields, including whether the final field
is variadic. Do not infer positionals from directory structure or prose. Use
named values for optional or uncommon settings:

```text
skill register-mcp
web search 'latest Rust release'
history search 'McpRuntime'
bash 'cargo test --all-targets'
close 01ABC
```

Named values remain the unambiguous general form:

```text
read src/main.rs line_offset=80
send 01ABC mode=steer
wait handles='["01ABC","j_01DEF"]'
mcp github search_code query='CommandRegistry' limit=20
```

### Standard Input

Represent stdin as `Option<String>`, not as an empty-string default. Omitted and
present-but-empty input have different meanings; an empty file write is valid.
Never trim, normalize, shell-expand, or otherwise rewrite stdin.

Each command declares one of:

- `forbidden`: reject a supplied stdin value;
- `optional`: accept stdin according to that command's contract;
- `required`: reject the call when stdin is absent.

Some commands may accept one value from either a command argument or stdin for
convenience, but they must reject calls that provide both. This is appropriate
for:

```text
bash 'cargo test'
bash                         # multiline script arrives on stdin
```

Do not generalize stdin into multiple named streams or an attachment protocol.
One opaque UTF-8 payload is enough for the current need.

### Compiled Form

The compiler should produce one provider-independent value similar to:

```rust
pub struct CompiledInvocation {
    pub route: CommandRoute,
    pub display_name: String,
    pub arguments: serde_json::Value,
}
```

`route` must be deterministic and must not be reconstructed from prose.
`display_name` is bounded observability metadata such as `patch` or `send`; it
must not become a second routing authority.

The exact provider `{"command": ..., "stdin": ...}` argument string remains in
the durable assistant message. Compile only at the execution boundary, as
Fiasco currently does for ordinary tool arguments.

## Candidate A Route Map

This map is the implementation target, not a requirement to create one Rust
module per row. Flat routes are preferred because the outer provider tool
already supplies one namespace.

| Current provider tool | Proposed command | stdin contract |
| --- | --- | --- |
| `read` | `read <path> [line_offset=N] [byte_offset=N]` | forbidden |
| complete `write` | `write <path>` | required; complete UTF-8 content |
| replacement `write` | `patch` | required; one patch payload |
| `bash` | `bash [command]` | optional alternative to positional command |
| `load_skill` | `skill <name>` | forbidden |
| `web_search` | `web search <query> [count=N]` | forbidden |
| `delegate` | `agent <name>` | required prompt |
| `send_message` | `send <handle> mode=steer|followup` | required message |
| `list_handles` | `handles [handle ...] [include_closed=true]` | forbidden |
| `inspect` | `inspect <handle> [...]` | forbidden |
| `wait` | `wait [handle ...]` | forbidden |
| `stop` | `stop <handle>` | forbidden |
| `close` | `close <handle>` | forbidden |
| `history_search` | `history search <pattern> [...]` | forbidden |
| `history_read` | `history read <ref> [...]` | forbidden |
| fixed `mcp` tool | `mcp <source> <remote-tool> [name=value ...]` | forbidden initially |

Check the real schemas before finalizing signatures. The table intentionally
omits optional fields that the current `tool.yaml` files already define.

## File Mutation Contract

### `write`

`write <path>` is an explicit complete replacement:

- stdin is required but may be the empty string;
- preserve the current parent-directory creation behavior;
- preserve permissions of an existing target;
- preserve atomic temporary-file replacement;
- continue to reject non-UTF-8 content;
- retain the existing path resolution and symlink behavior;
- return the same concise byte-count confirmation.

### `patch`

The preferred surface is the familiar:

```text
*** Begin Patch
*** Update File: path
@@
 unchanged context
-old text
+new text
*** End Patch
```

For the first implementation:

- support multiline changes and multiple hunks for one UTF-8 file;
- preserve current file locking, path resolution, BOM, line endings,
  permissions, and atomic replacement;
- apply every hunk against the original file before writing;
- require unique, non-overlapping matches;
- preserve only the current conservative line/indentation normalization;
- fail the whole call before writing if any hunk is invalid or ambiguous;
- support `Add File` only if it can reuse the complete-write path without
  weakening the contract;
- do not add fuzzy similarity matching or line-number guessing;
- do not invoke `git apply`, the system `patch` binary, or a shell;
- do not add multi-file transactions, delete, rename, or move semantics without
  discussing the expanded destructive and rollback behavior with the
  maintainer.

The exact patch grammar needs one focused design pass before implementation.
Prefer a small, model-familiar subset over either a novel DSL or a partial
emulation of every `git apply` behavior. Add parser fixtures before connecting
the parser to file writes.

## Candidate A Command Discovery And Prompting

The outer provider tool schema should stay fixed. It must explain only:

- the command grammar;
- the stdin rule;
- that one call represents one operation;
- that sibling calls execute concurrently;
- how to discover enabled commands.

Do not move all current tool prose into that one description.

Build a deterministic, compact catalog from the enabled command registry and
place it in the initial runtime reminder, for example:

```text
<available-commands>
- read: bounded text and image reads
- write: complete UTF-8 file replacement; content via stdin
- patch: conservative multiline edits; patch via stdin
- agent: start an isolated reusable agent; prompt via stdin
- send: steer or follow up an agent; message via stdin
- web search: search the public web
- ...
</available-commands>
```

The catalog must:

- include only commands that are actually executable in the frozen run;
- be sorted deterministically;
- remain concise;
- remove both capability and guidance when a command is ablated;
- distinguish optional commands such as web search and MCP.

Detailed per-command guidance should remain beside its handler in a typed asset.
A shared `help [namespace [verb]]` command may render those assets for both the
model and `fiasco invoke` CLI. If help is implemented, it must read the same
registry metadata used by compilation rather than creating a parallel manual.

Do not invent a list/load state machine. Help is a pure deterministic
projection of the already frozen command registry.

## Candidate A Registry And Handler Shape

Do not begin by rewriting every handler. First prove the provider surface with
an adapter over the existing `ToolRegistry`:

1. Add a command spec that owns route tokens, argument schema, stdin policy,
   concise catalog text, detailed help, and the existing handler.
2. Generalize the reusable pieces of `src/mcp/command.rs`; do not copy its
   tokenizer and type conversion into a second compiler.
3. Compile a command into the JSON arguments already consumed by the existing
   Rust `Tool::execute`.
4. Register only the fixed outer `fiasco` adapter with the provider in the
   experimental surface.
5. Keep `ToolContext`, `RawToolOutput`, artifact persistence, and the direct
   batch unchanged.

Only after the ablation succeeds should the implementation decide whether
`Tool` should be renamed to `CommandHandler` and `tool.yaml` to `command.yaml`.
The final code should have one concept, but doing that rename before behavioral
validation would produce a large diff without proving the product idea.

Keep registry construction explicit and deterministic. Avoid plugin discovery,
inventory files, code generation, or a generic framework for hypothetical
command sources.

Organize code by dependency and ownership without deriving routes from paths.
For example, the existing handle family may keep one registration function
while exposing flat routes:

```rust
handle::register(&mut commands, handles)?;

commands.register(["send"], SendCommand::new(handles.clone()))?;
commands.register(["handles"], ListHandlesCommand::new(handles.clone()))?;
commands.register(["wait"], WaitCommand::new(handles.clone()))?;
commands.register(["close"], CloseCommand::new(handles))?;
```

Likewise, `history::register` may expose `history search` and `history read`.
Family registration does not imply a model-visible family prefix. Every route
must be declared explicitly; moving a source file must not change the product
surface.

MCP should mount under the same registry as one dynamic command family. Keep
its artifact loader and exact captured schema as the authority for remote
names and arguments.

## Candidate A CLI Contract

Add one authoring/debugging path that uses the exact runtime compiler:

```bash
fiasco --workspace <workspace> invoke "read src/main.rs"
fiasco --workspace <workspace> invoke "write notes.md" < notes.md
fiasco --workspace <workspace> invoke "patch" <<'PATCH'
...
PATCH
```

Requirements:

- the CLI must not load a model provider;
- use the same workspace/config/optional-capability assembly needed by the
  runtime;
- read stdin to EOF when the selected command requires it;
- for optional stdin, read it only when stdin is redirected or explicitly
  requested so an interactive call does not block unexpectedly;
- write `RawToolOutput` bytes directly and preserve error status behavior;
- expose a compile-only/debug projection if it materially helps tests, but do
  not create separate parsers for `compile` and `invoke`;
- keep `mcp capture` and `mcp check` as MCP artifact-authoring commands;
- remove redundant `mcp compile` and `mcp call` after unified `invoke` covers
  them and tests have migrated.

## Execution And Persistence Invariants

The implementation must preserve:

- exact raw provider arguments in the durable assistant message;
- one ordinary tool result per call id;
- concurrent execution of one assistant call batch;
- original call-order result commits;
- one shared foreground window;
- promotion of only unfinished exact futures;
- artifact limiting and native image attachments;
- hook invocation and lifecycle events;
- ordinary tool failures as tool results rather than model-response failures;
- root/child use of the same frozen selected capability surface;
- compaction requests using the same frozen provider schema set while never
  executing tool calls.

Under Candidate A, the provider-visible tool name in persisted calls becomes
`fiasco`. Command text already makes the intended operation inspectable. If
hooks, events, or promoted-tool display names need a canonical operation label,
derive one from `CompiledInvocation.display_name`. Keep it display metadata; do
not add a second durable routing log.

Under Candidate B, real CLI calls appear as ordinary `bash` calls. Do not add a
parallel durable command log merely to recover the semantic subcommand; the
exact Bash command and ordinary output already remain in the transcript.

Do not change crash recovery, message-tail repair, handle durability, or agent
thread resume as part of this work.

## Capability Fingerprint And Resume

Today `tool_schema_sha256` hashes the sorted provider `ToolSpec` list. Under
Candidate A, hashing only the fixed outer `fiasco` schema would miss changes to
executable commands.

Under Candidate A, the new fingerprint must cover:

- the fixed provider tool schema;
- sorted built-in command routes and their compile-time argument/stdin specs;
- enabled optional command families;
- configured MCP artifact command schemas using the same stable information
  that compilation consumes.

Do not hash prose, absolute paths, credentials, live MCP results, or handler
implementation bytes. The fingerprint protects resume from a changed
executable capability contract; it is not an artifact-integrity or build hash.

Candidate B does not need a hidden hash of every external CLI subcommand because
those commands are invoked as Bash text rather than exposed runtime
capabilities. Any native process-local adapter still needs its exact executable
contract in the run fingerprint.

Renaming `tool_schema_sha256` to a broader capability name is optional. Do not
add migration machinery solely for unreleased run metadata. Document the
chosen cutover behavior.

## Implementation Checklist

### Phase 0: Record The Baseline

- [ ] Re-read `AGENTS.md`, the current MCP ADR, and current tool manifests.
- [ ] Confirm the checkout is based on or intentionally supersedes `a269e98`.
- [ ] Measure serialized provider tool-schema bytes, not only YAML source bytes.
- [ ] Save representative pre-change `qwen3.7-max` trajectories and token usage.
- [ ] Select tasks covering file editing, shell/test execution, agent
      orchestration, history after compaction, image/artifact output, and MCP.
- [ ] Confirm the MVP patch grammar with the maintainer.

### Phase 1: Bash-First Stateless CLI Spike

- [ ] Select the smallest stateless CLI slice: `web search`,
      `history search/read`, and `skill load`.
- [ ] Implement those as real `fiasco` CLI subcommands without loading a model
      provider.
- [ ] Reuse the same focused domain handlers as the current native tools; do not
      fork execution logic for the CLI.
- [ ] Call the new commands only through the existing `bash` tool in
      representative `qwen3.7-max` trajectories.
- [ ] Test ordinary quoting, redirected stdin where relevant, non-zero exits,
      large output, and config/environment inheritance.
- [ ] Measure shell/CLI startup cost, syntax correction turns, context usage,
      and result readability.
- [ ] Do not route live handles, active agent work, or promoted futures through
      a child CLI.
- [ ] Test MCP cold-call startup separately before proposing removal of the
      in-process MCP adapter.

### Phase 2: Choose The Product Surface

- [ ] Review the Bash-first results with the maintainer.
- [ ] Choose Candidate A, Candidate B, or an explicitly agreed small hybrid.
- [ ] If Candidate A or a hybrid retains one process-local router, confirm its
      provider-visible name and exact command scope.
- [ ] Record which commands remain native because they provide bounded reads,
      precise mutation, native images, live handles, or persistent sessions.
- [ ] Do not proceed with IPC, inherited control descriptors, or Bash prefix
      interception unless external control of a live run becomes a separate
      product goal.

### Phase 3A: Candidate A Shared Compiler

Complete this phase only if the accepted surface includes an in-process command
router.

- [ ] Extract reusable tokenization and top-level type conversion from
      `src/mcp/command.rs`.
- [ ] Introduce `CommandSpec`, stdin policy, `CompiledInvocation`, and a
      deterministic registry.
- [ ] Add route collision checks and deterministic catalog ordering.
- [ ] Add explicit positional and optional variadic fields to command specs.
- [ ] Add pure compile tests before connecting handlers.
- [ ] Register MCP through the shared compiler without changing MCP call
      behavior.
- [ ] Ensure compile errors are concise ordinary tool errors.
- [ ] Map each current built-in `Tool` to the proposed command route.
- [ ] Map stdin into the handler's current typed arguments without changing its
      execution semantics.
- [ ] Add the fixed outer provider tool.
- [ ] Add the compact enabled-command catalog to the runtime reminder.
- [ ] Keep the existing direct batch and `RawToolOutput` pipeline.
- [ ] Update the capability fingerprint.
- [ ] Add an experimental way to choose old or unified provider surfaces for
      ablation only; do not document it as a permanent compatibility feature.

### Phase 3B: Candidate B CLI Expansion

Complete this phase only if the accepted surface is Bash-first.

- [ ] Expand real CLI commands only for stateless capabilities that benefited
      from the spike.
- [ ] Keep process-local runtime controls in the smallest agreed native adapter
      or existing controls.
- [ ] Keep native `read` and file mutation paths where they materially improve
      bounded access, images, or multiline edits.
- [ ] Keep MCP in process unless reconnect-per-call behavior and latency were
      explicitly accepted.
- [ ] Remove native schemas only for capabilities whose Bash CLI replacement
      passed the same trajectories.

### Phase 4: Unified CLI And File Commands

- [ ] Under Candidate A, implement `fiasco invoke` over the exact runtime
      compiler, registry, handler, and output renderer.
- [ ] Under Candidate B, retain conventional focused CLI subcommands rather
      than introducing `invoke` solely for symmetry.
- [ ] Cover redirected stdin, empty stdin, required stdin, and interactive
      no-stdin behavior.
- [ ] Migrate MCP compile/call integration coverage only if the selected
      surface actually replaces those paths.
- [ ] Split complete replacement into `write`.
- [ ] Implement and unit-test the agreed patch parser without file writes.
- [ ] Connect `patch` to current locking, matching, line-ending, permission,
      path, and atomic-replacement machinery.
- [ ] Reject unsupported multi-file or destructive sections clearly.
- [ ] Replace the old structured `write` provider contract only after parity is
      demonstrated.

### Phase 5: Behavioral Ablation

- [ ] Run the same tasks with the current surface and the selected candidate.
- [ ] Use the configured `qwen3.7-max` profile; disclose any provider deviation.
- [ ] Compare first-call syntax success, correction turns, total model calls,
      task outcome, input tokens, cached tokens, and output tokens.
- [ ] Verify multiple sibling calls in one assistant response still run
      concurrently, whether they are `fiasco` or `bash` calls.
- [ ] Inspect `messages.jsonl`, `events.jsonl`, final output, handle notices, and
      artifact contents for substantive runs.
- [ ] Compare persistent in-process MCP calls with cold CLI calls if Candidate B
      proposes moving MCP behind Bash.
- [ ] Record regressions rather than compensating with speculative parser
      fallbacks.
- [ ] Discuss any material reliability loss or extra discovery turns with the
      maintainer before cutover.

### Phase 6: Cutover And Cleanup

- [ ] Remove the old provider-visible built-in specs and temporary surface
      switch.
- [ ] Remove the fixed standalone `mcp` provider adapter only if the chosen
      replacement preserves required session behavior.
- [ ] Delete redundant parsers, registrations, manifests, and CLI commands.
- [ ] Rename internal `Tool` concepts only where the final ownership becomes
      clearer.
- [ ] Update README, architecture, runtime model, source map, prompt docs, and
      design choices.
- [ ] Add an ADR that refines or supersedes ADRs 0014, 0015, 0016, 0024, 0039,
      and 0049 as appropriate.
- [ ] Update `AGENTS.md` invariants only after the implementation and ADR agree.
- [ ] Verify every embedded command/help asset is present in
      `cargo package --list`.
- [ ] Obtain an independent review focused on parser ambiguity, lost execution
      semantics, prompt size, and unnecessary compatibility code.

## Test Matrix

### Compiler

Candidate A only:

- quoted spaces and newlines;
- declared positional, optional variadic, and named values;
- strings that resemble booleans or numbers but are schema-declared strings;
- booleans, signed/unsigned integers, finite numbers, nulls, arrays, and
  objects;
- duplicate, missing, and unknown arguments;
- malformed shell quoting and malformed compound JSON;
- route collisions and unknown namespaces/verbs;
- stdin forbidden, optional, required, omitted, empty, and non-empty;
- argument/stdin mutual exclusion;
- exact stdin preservation;
- rejection of apparent pipelines, separators, and redirections.

### File Mutation

- create and empty complete writes;
- existing file replacement with mode preservation;
- one and multiple patch hunks;
- ambiguous, missing, overlapping, and no-op hunks;
- LF, CRLF, CR, BOM, and mixed-line-ending rejection;
- UTF-8 boundary behavior;
- symlink behavior and workspace-relative/absolute paths;
- failure before mutation when any hunk is invalid;
- cleanup of temporary files after failure;
- concurrent calls targeting the same file;
- explicit rejection of unsupported delete/move/multi-file patches.

### Runtime

- two or more sibling calls to the selected outer tool (`fiasco` under
  Candidate A or `bash` under Candidate B);
- completion in a different order from call order;
- one malformed command beside valid siblings;
- foreground timeout and exact-future promotion;
- background artifact delivery;
- large and binary output;
- native image attachments;
- tool-before and tool-after hooks;
- event and inspect readability;
- Candidate A command catalog persistence through compaction;
- root and child schema equality;
- resume rejection after a meaningful command-contract change.

### Command Families

- all current read offsets and image modality checks;
- shell success, failure, signal, large output, and process-group cancellation;
- skill loading and resource-root projection;
- optional web search configuration;
- delegate, steer, followup, list, inspect, wait-any, stop, close, and idle reuse;
- history search/read after compaction, including artifact-backed matches;
- MCP capture/check plus text, structured, rich, error, and stale-catalog calls.

### Providers

- every provider request contains exactly the native tool set selected after
  ablation;
- Candidate A exposes one fixed `fiasco` schema across OpenAI Responses,
  OpenAI Chat, and Anthropic;
- Candidate B exposes the same selected fast native schemas across those
  providers and no schemas for CLI-only capabilities;
- fragmented tool-argument streams still reconstruct exact outer arguments;
- malformed outer JSON remains one ordinary tool failure;
- compaction rejects any returned tool call without executing it.

## Required Verification

Run for the final code change:

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo package --list
```

Also run:

- a headless `qwen3.7-max` file-editing trajectory using native `read`,
  `patch`, and `bash`;
- a Bash-first trajectory invoking the selected stateless Fiasco CLI commands;
- a multi-agent trajectory using delegate, send, wait-any, and close;
- an MCP trajectory through every MCP path proposed for the final surface;
- an artifact/image trajectory when the configured model supports images;
- redirected-stdin CLI tests, including a real shell heredoc;
- transcript inspection for each substantive smoke.

Report which paths were not exercised with live credentials.

## Acceptance Criteria

The task is complete only when:

- the final provider surface matches the explicitly selected candidate and is
  smaller than the baseline for measured reasons;
- every capability exposed through both model and CLI shares one domain
  execution path, without duplicate business logic;
- detailed command schemas no longer occupy every provider request;
- any enabled-command catalog is deterministic, compact, and ablation-correct;
- multiline payloads use stdin and the harness contains no heredoc parser;
- `patch` supports the agreed conservative multiline edit contract;
- internal handlers remain focused and no monolithic execution match absorbs
  their domain logic;
- Bash-launched CLI commands are limited to capabilities that can be correctly
  reconstructed outside the live parent process;
- live handles, active agent work, promoted futures, and persistent sessions do
  not depend on hidden IPC or command-prefix interception;
- concurrent batch, ordered results, background promotion, handles, artifacts,
  images, hooks, compaction, and resume boundaries retain their behavior;
- real `qwen3.7-max` trajectories show acceptable reliability and a measured
  context reduction;
- temporary dual-surface code and genuinely redundant paths are removed;
- documentation and a final ADR describe the implementation that actually
  shipped;
- the full verification suite passes and the worktree contains no generated
  runs, credentials, or build output.

## Stop And Discuss

Stop and ask the maintainer before:

- adding shell execution semantics to the command parser;
- adding a Unix socket, RPC server, inherited control descriptor, or Bash
  command-prefix interception to make a child CLI control the live parent;
- adding parser recovery, fuzzy correction, aliases, or compatibility fallbacks
  because a trajectory produced an invalid command;
- expanding patch into multi-file transactions, delete, move, or rename;
- changing background, handle, crash recovery, or message durability behavior;
- introducing a second command catalog, documentation store, or persisted
  routing log;
- hiding a reliability regression by making the model prompt substantially
  larger;
- keeping both provider surfaces after the ablation;
- adding a generic plugin framework or dynamic command hot reload.

The design goal is a smaller and more legible execution surface, not a second
shell, an RPC framework, or a new recovery subsystem.
