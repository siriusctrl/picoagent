# Source Map

- `src/agent/runner.rs`: the only model/tool loop.
- `src/agent/runner/lifecycle.rs`: run creation, profiles, and finalization.
- `src/agent/runner/recovery.rs`: resume validation and durable result injection.
- `src/agent/compaction.rs`: local compaction planning, state calls, and
  active-context assembly.
- `src/agent/tool_execution.rs`: shared direct/background ordinary-tool hooks,
  events, foreground promotion, and artifact-backed output persistence.
- `src/agent/types.rs`: runner configuration, request, and result contracts.
- `src/agent/task.rs`: background task coordination and delivery state.
- `src/agent/task/control.rs`: status, message inspection, steering, and targeted stop.
- `src/agent/task/execution.rs`: background tool and child-run execution.
- `src/agent/task/lifecycle.rs`: failed task state and events.
- `src/agent/task/record.rs`: persisted background task state and model envelope.
- `src/agent/task/recovery.rs`: task reload, child reconciliation, and cancellation.
- `src/storage/input.rs`: durable pending user input used by non-interrupting child steering.
- `src/agent/context.rs`: deterministic prompt framing and dynamic reminder assembly.
- `src/prompts.rs`: typed access to the embedded agent prompt registry.
- `prompts/agents.yaml`: folded agent instructions for every fixed profile.
- `src/model/mod.rs`: canonical messages, tools, requests, responses, and provider trait.
- `src/model/openai_oauth.rs`: OAuth provider orchestration and 401 retry.
- `src/model/openai_compatible.rs`: Responses/Chat provider facade.
- `src/model/openai_request.rs`: OpenAI request serialization and continuation replay.
- `src/model/openai_stream.rs`: shared Responses/Chat SSE parsing.
- `src/model/openai_oauth_credentials.rs`: auth-file, Codex import, and JWT helpers.
- `src/model/openai_oauth_device.rs`: device-code request and polling.
- `src/model/anthropic_compatible.rs`: Messages adapter.
- `src/tools/mod.rs`: tool contract and deterministic sorted registry.
- `src/tools/manifest.rs`: strict parser for embedded local `tool.yaml`
  contracts.
- `src/tools/assembly.rs`: the single process/run tool-assembly path.
- `src/tools/{bash,delegate,history_read,history_search,load_skill,read,task_inspect,task_status,task_steer,task_stop,task_wait,web_search,write}/`:
  flat local adapters with a typed compile-time `tool.yaml` containing purpose,
  return guidance, and input schema beside their Rust arguments, validation,
  and execution.
- `src/trajectory.rs` and `src/trajectory/`: provider-neutral history reader
  contracts plus local message/artifact search.
- `src/artifact.rs`: versioned artifact envelope and spill.
- `src/artifact/preview.rs`: bounded UTF-8-safe file and byte previews.
- `src/artifact/model-instruction.md`: compact model guidance for inspecting a
  spilled result.
- `src/storage/mod.rs`: run directories, metadata, events, and shared JSON
  persistence helpers.
- `src/storage/trajectory.rs`: classified append-only messages and
  compacted-history loading.
- `src/skills/mod.rs`: Agent Skills metadata discovery and body/path loading;
  the model-facing adapter is in `src/tools/load_skill/`.
- `src/mcp.rs`: rmcp stdio client and tool adapters.
- `src/hooks.rs`: deterministic command-hook pipeline.
- `src/memory.rs`: user/project Markdown memory paths and reminder text.
- `src/config.rs`: TOML configuration.
- `src/events.rs`: runtime event contract and sinks.
- `src/cli.rs`: CLI command schema.
- `src/main.rs`: headless composition root.
