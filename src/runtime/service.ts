import { join } from 'node:path';
import { createRuntimeContext } from './index.js';
import type { ExecutionBackend } from '../core/execution.js';
import type { MutableFilesystem } from '../core/filesystem.js';
import type { AgentPresetId } from '../core/types.js';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.js';
import { RuntimeConflictError, RuntimeEngine, RuntimeValidationError } from './engine.js';
import { LocalExecutionBackend } from './local-execution-backend.js';
import { FileRuntimeStore, InMemoryRuntimeStore } from './runtime-store.js';
import type { RunEvent, RunSnapshot, RunStatus, RuntimeStore, SessionRecord, SessionSnapshot } from './store.js';

export interface RuntimeServiceOptions {
  cwd?: string;
  filesystem?: MutableFilesystem;
  executionBackend?: ExecutionBackend;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

export class RuntimeNotFoundError extends Error {
  readonly status = 404;
}

export class RuntimeService {
  private readonly store: RuntimeStore;
  private readonly engine: RuntimeEngine;

  constructor(options: RuntimeServiceOptions = {}) {
    const cwd = options.cwd ?? process.cwd();
    const filesystem = options.filesystem ?? new LocalWorkspaceFileSystem();
    const executionBackend = options.executionBackend ?? new LocalExecutionBackend();
    const runtimeRoot = options.runtimeRoot ?? join(cwd, '.pico', 'runtime');
    const persistentRuntime = options.persistentRuntime ?? true;
    const runtimeContext = createRuntimeContext(cwd);

    this.store = persistentRuntime
      ? new FileRuntimeStore(runtimeRoot)
      : new InMemoryRuntimeStore();
    this.engine = new RuntimeEngine({
      cwd,
      filesystem,
      executionBackend,
      runtimeContext,
      store: this.store,
    });
  }

  async createSession(agent: AgentPresetId = 'ask'): Promise<SessionRecord> {
    return this.engine.createSession(agent);
  }

  getSession(id: string): SessionRecord {
    const session = this.store.getSession(id);
    if (!session) {
      throw new RuntimeNotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  getRunSnapshot(id: string): RunSnapshot {
    const run = this.store.getRunSnapshot(id);
    if (!run) {
      throw new RuntimeNotFoundError(`Run ${id} not found`);
    }

    return run;
  }

  getSessionSnapshot(id: string): SessionSnapshot {
    const session = this.store.getSessionSnapshot(id);
    if (!session) {
      throw new RuntimeNotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  async createStandaloneRun(prompt: string, agent: AgentPresetId): Promise<RunSnapshot> {
    return this.engine.createStandaloneRun(prompt, agent);
  }

  async createSessionRun(sessionId: string, prompt: string, agent?: AgentPresetId): Promise<RunSnapshot> {
    return this.engine.createSessionRun(this.getSession(sessionId), prompt, agent);
  }

  setSessionAgent(sessionId: string, agent: AgentPresetId): SessionSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new RuntimeConflictError(`Session ${sessionId} already has an active run`);
    }

    this.store.setSessionAgent(sessionId, agent);
    return this.getSessionSnapshot(sessionId);
  }

  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } {
    const events = this.store.getRunEvents(runId);
    if (!events) {
      throw new RuntimeNotFoundError(`Run ${runId} not found`);
    }

    return events;
  }

  subscribeToRun(runId: string, listener: (event: RunEvent) => void): () => void {
    const unsubscribe = this.store.subscribeToRun(runId, listener);
    if (!unsubscribe) {
      throw new RuntimeNotFoundError(`Run ${runId} not found`);
    }

    return unsubscribe;
  }

  listSessionResources(sessionId: string, path = '.'): string[] {
    this.getSession(sessionId);
    const entries = this.store.listSessionResources(sessionId, path);
    if (!entries) {
      throw new RuntimeNotFoundError(`Session resource directory not found: ${path}`);
    }

    return entries;
  }

  readSessionResource(sessionId: string, path: string): string {
    this.getSession(sessionId);
    const content = this.store.readSessionResource(sessionId, path);
    if (content === undefined) {
      throw new RuntimeNotFoundError(`Session resource not found: ${path}`);
    }

    return content;
  }

  compactSession(sessionId: string, keepLastMessages = 8) {
    if (!Number.isInteger(keepLastMessages) || keepLastMessages < 0) {
      throw new RuntimeValidationError('keepLastMessages must be a non-negative integer');
    }

    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new RuntimeConflictError(`Session ${sessionId} already has an active run`);
    }

    const result = this.store.compactSession(sessionId, keepLastMessages);
    if (!result) {
      throw new RuntimeNotFoundError(`Session ${sessionId} not found`);
    }

    return {
      checkpoint: result,
      session: this.getSessionSnapshot(sessionId),
    };
  }
}
