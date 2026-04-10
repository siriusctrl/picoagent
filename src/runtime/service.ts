import { join } from 'node:path';
import { createRuntimeContext } from './index.js';
import type { ExecutionBackend } from '../core/execution.js';
import type { MutableFilesystem } from '../core/filesystem.js';
import type { AgentPresetId } from '../core/types.js';
import type { NamespaceMount } from '../fs/namespace.js';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.js';
import { RuntimeConflictError, RuntimeEngine, RuntimeValidationError } from './engine.js';
import type { SessionStore } from './store.js';
import { LocalExecutionBackend } from './local-execution-backend.js';
import { FileRuntimeStore, InMemoryRuntimeStore } from './runtime-store.js';
import { StoreBackedSessionStore } from './store-backed-session-store.js';
import type { RunEvent, RunSnapshot, RunStatus, RuntimeStore, SessionRecord, SessionSnapshot } from './store.js';

export interface RuntimeServiceOptions {
  cwd?: string;
  filesystem?: MutableFilesystem;
  mounts?: NamespaceMount[];
  executionBackend?: ExecutionBackend;
  sessionStore?: SessionStore;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

export class RuntimeNotFoundError extends Error {
  readonly status = 404;
}

export class RuntimeService {
  private readonly store: RuntimeStore;
  private readonly sessionStore: SessionStore;
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
    this.sessionStore = options.sessionStore ?? new StoreBackedSessionStore(this.store);
    this.engine = new RuntimeEngine({
      cwd,
      filesystem,
      mounts: options.mounts,
      executionBackend,
      runtimeContext,
      runStore: this.store,
      sessionStore: this.sessionStore,
    });
  }

  async createSession(agent: AgentPresetId = 'ask'): Promise<SessionRecord> {
    return this.engine.createSession(agent);
  }

  async getSession(id: string): Promise<SessionRecord> {
    const session = await this.sessionStore.getSession(id);
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

  async getSessionSnapshot(id: string): Promise<SessionSnapshot> {
    const session = await this.sessionStore.getSessionSnapshot(id);
    if (!session) {
      throw new RuntimeNotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  async createStandaloneRun(prompt: string, agent: AgentPresetId): Promise<RunSnapshot> {
    return this.engine.createStandaloneRun(prompt, agent);
  }

  async createSessionRun(sessionId: string, prompt: string, agent?: AgentPresetId): Promise<RunSnapshot> {
    return this.engine.createSessionRun(await this.getSession(sessionId), prompt, agent);
  }

  async setSessionAgent(sessionId: string, agent: AgentPresetId): Promise<SessionSnapshot> {
    const session = await this.getSession(sessionId);
    if (session.activeRunId) {
      throw new RuntimeConflictError(`Session ${sessionId} already has an active run`);
    }

    await this.sessionStore.setSessionAgent(sessionId, agent);
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

  async listSessionResources(sessionId: string, path = '.'): Promise<string[]> {
    await this.getSession(sessionId);
    const entries = await this.sessionStore.listSessionResources(sessionId, path);
    if (!entries) {
      throw new RuntimeNotFoundError(`Session resource directory not found: ${path}`);
    }

    return entries;
  }

  async readSessionResource(sessionId: string, path: string): Promise<string> {
    await this.getSession(sessionId);
    const content = await this.sessionStore.readSessionResource(sessionId, path);
    if (content === undefined) {
      throw new RuntimeNotFoundError(`Session resource not found: ${path}`);
    }

    return content;
  }

  async compactSession(sessionId: string, keepLastMessages = 8) {
    if (!Number.isInteger(keepLastMessages) || keepLastMessages < 0) {
      throw new RuntimeValidationError('keepLastMessages must be a non-negative integer');
    }

    const session = await this.getSession(sessionId);
    if (session.activeRunId) {
      throw new RuntimeConflictError(`Session ${sessionId} already has an active run`);
    }

    const result = await this.sessionStore.compactSession(sessionId, keepLastMessages);
    if (!result) {
      throw new RuntimeNotFoundError(`Session ${sessionId} not found`);
    }

    return {
      checkpoint: result,
      session: await this.getSessionSnapshot(sessionId),
    };
  }
}
