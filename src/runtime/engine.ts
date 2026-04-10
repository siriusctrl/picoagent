import { randomUUID } from 'node:crypto';
import { buildSessionControlSnapshot, computeControlVersion, SessionControlSnapshot } from './control-snapshot.js';
import type { ExecutionBackend } from '../core/execution.js';
import type { MutableFilesystem } from '../core/filesystem.js';
import { FileViewAccess } from '../core/file-view.js';
import { runAgentLoop } from '../core/loop.js';
import { AgentPresetId, AssistantMessage, Message } from '../core/types.js';
import { Namespace, type NamespaceMount } from '../fs/namespace.js';
import { createProvider } from '../providers/index.js';
import { createRuntimeFileViewAccess } from './file-view-access.js';
import { RuntimeContext } from './index.js';
import type {
  EmittedRunEvent,
  RunRecord,
  RunStore,
  RunSnapshot,
  SessionRecord,
  SessionStore,
} from './store.js';

export class RuntimeConflictError extends Error {
  readonly status = 409;
}

export class RuntimeValidationError extends Error {
  readonly status = 400;
}

function nowIso(): string {
  return new Date().toISOString();
}

function assistantText(message: AssistantMessage): string {
  return message.content
    .filter((item): item is { type: 'text'; text: string } => item.type === 'text')
    .map((item) => item.text)
    .join('');
}

export interface RuntimeEngineOptions {
  cwd: string;
  filesystem: MutableFilesystem;
  executionBackend: ExecutionBackend;
  runtimeContext: RuntimeContext;
  runStore: RunStore;
  sessionStore: SessionStore;
  mounts?: NamespaceMount[];
}

export class RuntimeEngine {
  private readonly namespace: Namespace;

  constructor(private readonly options: RuntimeEngineOptions) {
    this.namespace = new Namespace([
      {
        name: 'workspace',
        filesystem: options.filesystem,
        root: '.',
        writable: true,
        executable: true,
      },
      ...(options.mounts ?? []),
    ]);
  }

  async createSession(agent: AgentPresetId = 'ask'): Promise<SessionRecord> {
    const control = await this.buildControlSnapshot(this.options.cwd);
    const session: SessionRecord = {
      id: randomUUID(),
      cwd: this.options.cwd,
      roots: [this.options.cwd],
      agent,
      controlVersion: control.controlVersion,
      controlConfig: control.config,
      systemPrompts: control.systemPrompts,
      createdAt: nowIso(),
      runIds: [],
      messages: [],
      checkpoints: [],
    };
    return this.options.sessionStore.createSession(session);
  }

  async createStandaloneRun(prompt: string, agent: AgentPresetId): Promise<RunSnapshot> {
    const control = await this.buildControlSnapshot(this.options.cwd);
    const run = await this.createRun(prompt, agent);
    this.startRun(run, control);
    return this.requireRunSnapshot(run.id);
  }

  async createSessionRun(session: SessionRecord, prompt: string, agent?: AgentPresetId): Promise<RunSnapshot> {
    if (session.activeRunId) {
      throw new RuntimeConflictError(`Session ${session.id} already has an active run`);
    }

    const control = await this.ensureSessionControlSnapshot(session);
    const latestSession = await this.options.sessionStore.getSession(session.id);
    if (!latestSession) {
      throw new Error(`Session ${session.id} not found`);
    }

    if (latestSession.activeRunId) {
      throw new RuntimeConflictError(`Session ${session.id} already has an active run`);
    }

    const run = await this.createRun(prompt, agent ?? latestSession.agent, latestSession.id);
    await this.options.sessionStore.attachRunToSession(latestSession.id, run.id);
    this.startRun(run, control, latestSession);
    return this.requireRunSnapshot(run.id);
  }

  private requireRunSnapshot(runId: string): RunSnapshot {
    const snapshot = this.options.runStore.getRunSnapshot(runId);
    if (!snapshot) {
      throw new Error(`Run ${runId} not found`);
    }

    return snapshot;
  }

  private async createRun(prompt: string, agent: AgentPresetId, sessionId?: string): Promise<RunRecord> {
    const run = this.options.runStore.createRun({
      id: randomUUID(),
      sessionId,
      agent,
      prompt,
      status: 'running',
      output: '',
      createdAt: nowIso(),
      events: [],
    });
    if (sessionId) {
      await this.options.sessionStore.createRun(run);
    }

    return run;
  }

  private async buildControlSnapshot(workspaceRoot: string, controlVersion?: string): Promise<SessionControlSnapshot> {
    return buildSessionControlSnapshot(
      workspaceRoot,
      this.options.runtimeContext.registry,
      this.namespace.mount('workspace').filesystem,
      controlVersion,
    );
  }

  private async ensureSessionControlSnapshot(session: SessionRecord): Promise<SessionControlSnapshot> {
    const latestVersion = await computeControlVersion(session.cwd, this.namespace.mount('workspace').filesystem);
    if (latestVersion === session.controlVersion) {
      return {
        workspaceRoot: session.cwd,
        controlVersion: session.controlVersion,
        config: session.controlConfig,
        systemPrompts: session.systemPrompts,
      };
    }

    const refreshed = await this.buildControlSnapshot(session.cwd, latestVersion);
    await this.options.sessionStore.refreshSessionControl(session.id, {
      controlVersion: refreshed.controlVersion,
      controlConfig: refreshed.config,
      systemPrompts: refreshed.systemPrompts,
    });
    return refreshed;
  }

  private startRun(run: RunRecord, control: SessionControlSnapshot, session?: SessionRecord): void {
    void this.executeRun(run, control, session).catch(() => {
      // Run failures are captured in the run state and emitted as events.
    });
  }

  private async emit(runId: string, event: EmittedRunEvent): Promise<void> {
    const pendingEvent = {
      ...event,
      timestamp: nowIso(),
    };
    this.options.runStore.appendRunEvent(runId, pendingEvent);
    if (event.sessionId) {
      await this.options.sessionStore.appendRunEvent(runId, pendingEvent);
    }
  }

  private fileView(
    runId: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
  ): FileViewAccess {
    return createRuntimeFileViewAccess({
      namespace: this.namespace,
      sessionStore: this.options.sessionStore,
      executionBackend: this.options.executionBackend,
      runId,
      cwd,
      roots,
      signal,
      sessionId,
      validationError: (message) => new RuntimeValidationError(message),
    });
  }

  private async executeRun(run: RunRecord, control: SessionControlSnapshot, session?: SessionRecord): Promise<void> {
    const controller = new AbortController();
    let sessionFinalized = false;
    const startedAt = nowIso();
    this.options.runStore.updateRun(run.id, { startedAt });
    if (run.sessionId) {
      await this.options.sessionStore.updateRun(run.id, { startedAt });
    }
    await this.emit(run.id, {
      type: 'run_started',
      runId: run.id,
      sessionId: run.sessionId,
      agent: run.agent,
      prompt: run.prompt,
    });

    const conversation: Message[] = session
      ? [...session.messages, { role: 'user', content: run.prompt }]
      : [{ role: 'user', content: run.prompt }];

    const tools = this.options.runtimeContext.registry.forAgent(run.agent);
    const systemPrompt = control.systemPrompts[run.agent];
    const provider = createProvider(control.config);

    try {
      const finalMessage = await runAgentLoop(
        conversation,
        tools,
        provider,
        {
          runId: run.id,
          sessionId: session?.id,
          cwd: session?.cwd ?? this.options.cwd,
          roots: session?.roots ?? [this.options.cwd],
          controlRoot: control.workspaceRoot,
          agent: run.agent,
          signal: controller.signal,
          fileView: this.fileView(run.id, session?.cwd ?? this.options.cwd, session?.roots ?? [this.options.cwd], controller.signal, session?.id),
        },
        systemPrompt,
        {
          onTextDelta: async (text) => {
            const latestRun = this.options.runStore.getRun(run.id);
            if (!latestRun) {
              return;
            }

            this.options.runStore.updateRun(run.id, { output: latestRun.output + text });
            if (run.sessionId) {
              await this.options.sessionStore.updateRun(run.id, { output: latestRun.output + text });
            }
            await this.emit(run.id, {
              type: 'assistant_delta',
              runId: run.id,
              sessionId: run.sessionId,
              text,
            });
          },
          onToolStart: async (toolCall, tool) => {
            await this.emit(run.id, {
              type: 'tool_call',
              runId: run.id,
              sessionId: run.sessionId,
              title: tool?.title && typeof tool.title === 'string' ? tool.title : tool?.name ?? toolCall.name,
              toolCallId: toolCall.id,
              status: 'pending',
              kind: tool?.kind,
              rawInput: toolCall.arguments,
            });
          },
          onToolEnd: async (toolCall, _tool, result) => {
            await this.emit(run.id, {
              type: 'tool_call_update',
              runId: run.id,
              sessionId: run.sessionId,
              toolCallId: toolCall.id,
              title: result.title,
              status: result.message.isError ? 'failed' : 'completed',
              rawOutput: result.rawOutput,
              text: result.message.content,
            });
          },
        },
      );

      if (session) {
        await this.options.sessionStore.finishSessionRun(session.id, run.id, conversation);
        await this.options.sessionStore.clearSessionActiveRun(session.id, run.id);
        sessionFinalized = true;
      }

      this.options.runStore.updateRun(run.id, {
        output: assistantText(finalMessage),
        status: 'completed',
        finishedAt: nowIso(),
      });
      if (run.sessionId) {
        await this.options.sessionStore.updateRun(run.id, {
          output: assistantText(finalMessage),
          status: 'completed',
          finishedAt: nowIso(),
        });
      }
      await this.emit(run.id, {
        type: 'done',
        runId: run.id,
        sessionId: run.sessionId,
        output: assistantText(finalMessage),
      });
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);

      if (session && !sessionFinalized) {
        await this.options.sessionStore.clearSessionActiveRun(session.id, run.id);
        sessionFinalized = true;
      }

      this.options.runStore.updateRun(run.id, {
        status: 'failed',
        error: message,
        finishedAt: nowIso(),
      });
      if (run.sessionId) {
        await this.options.sessionStore.updateRun(run.id, {
          status: 'failed',
          error: message,
          finishedAt: nowIso(),
        });
      }
      await this.emit(run.id, {
        type: 'error',
        runId: run.id,
        sessionId: run.sessionId,
        message,
      });
    } finally {
      if (session && !sessionFinalized) {
        await this.options.sessionStore.clearSessionActiveRun(session.id, run.id);
      }
    }
  }
}
