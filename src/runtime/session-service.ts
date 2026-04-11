import { joinPath } from '../fs/path.ts';
import { FileRuntimeStore, InMemoryRuntimeStore } from './runtime-store.ts';
import { StoreBackedSessionStore } from './store-backed-session-store.ts';
import type { PendingRunEvent, RunRecord, RuntimeStore, SessionRecord, SessionSnapshot } from './store.ts';

function nowIso(): string {
  return new Date().toISOString();
}

export class SessionNotFoundError extends Error {
  readonly status = 404;
}

export class SessionConflictError extends Error {
  readonly status = 409;
}

export class SessionValidationError extends Error {
  readonly status = 400;
}

export interface SessionServiceOptions {
  cwd?: string;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

export class SessionService {
  readonly store: RuntimeStore;
  private readonly sessionStore: StoreBackedSessionStore;
  private readonly cwd: string;

  private constructor(options: SessionServiceOptions, store: RuntimeStore) {
    this.cwd = options.cwd ?? process.cwd();
    this.store = store;
    this.sessionStore = new StoreBackedSessionStore(this.store);
  }

  static async create(options: SessionServiceOptions = {}): Promise<SessionService> {
    const cwd = options.cwd ?? process.cwd();
    const runtimeRoot = options.runtimeRoot ?? joinPath(cwd, '.pico', 'runtime');
    const persistentRuntime = options.persistentRuntime ?? true;
    const store = persistentRuntime
      ? await FileRuntimeStore.create(runtimeRoot)
      : new InMemoryRuntimeStore();

    return new SessionService(options, store);
  }

  async createSession(): Promise<SessionRecord> {
    return this.sessionStore.createSession({
      id: crypto.randomUUID(),
      cwd: this.cwd,
      roots: [this.cwd],
      createdAt: nowIso(),
      runIds: [],
      messages: [],
      checkpoints: [],
    });
  }

  async createSessionRecord(record: SessionRecord): Promise<SessionRecord> {
    return this.sessionStore.createSession(record);
  }

  async createRunRecord(record: RunRecord): Promise<void> {
    await this.store.createRun(record);
  }

  async updateRunRecord(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): Promise<void> {
    await this.store.updateRun(runId, patch);
  }

  async appendRunEvent(runId: string, event: PendingRunEvent): Promise<void> {
    await this.store.appendRunEvent(runId, event);
  }

  async getSession(id: string): Promise<SessionRecord> {
    const session = await this.sessionStore.getSession(id);
    if (!session) {
      throw new SessionNotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  async getSessionSnapshot(id: string): Promise<SessionSnapshot> {
    const snapshot = await this.sessionStore.getSessionSnapshot(id);
    if (!snapshot) {
      throw new SessionNotFoundError(`Session ${id} not found`);
    }

    return snapshot;
  }

  async attachRunToSession(sessionId: string, runId: string): Promise<void> {
    await this.getSession(sessionId);
    await this.sessionStore.attachRunToSession(sessionId, runId);
  }

  async finishSessionRun(sessionId: string, runId: string, messages: SessionRecord['messages']): Promise<void> {
    await this.getSession(sessionId);
    await this.sessionStore.finishSessionRun(sessionId, runId, messages);
  }

  async clearSessionActiveRun(sessionId: string, runId: string): Promise<void> {
    await this.getSession(sessionId);
    await this.sessionStore.clearSessionActiveRun(sessionId, runId);
  }

  async listSessionResources(sessionId: string, resourcePath = '.'): Promise<string[]> {
    await this.getSession(sessionId);
    const entries = await this.sessionStore.listSessionResources(sessionId, resourcePath);
    if (!entries) {
      throw new SessionNotFoundError(`Session resource directory not found: ${resourcePath}`);
    }

    return entries;
  }

  async readSessionResource(sessionId: string, resourcePath: string): Promise<string> {
    await this.getSession(sessionId);
    const content = await this.sessionStore.readSessionResource(sessionId, resourcePath);
    if (content === undefined) {
      throw new SessionNotFoundError(`Session resource not found: ${resourcePath}`);
    }

    return content;
  }

  async compactSession(sessionId: string, keepLastMessages = 8) {
    if (!Number.isInteger(keepLastMessages) || keepLastMessages < 0) {
      throw new SessionValidationError('keepLastMessages must be a non-negative integer');
    }

    const session = await this.getSession(sessionId);
    if (session.activeRunId) {
      throw new SessionConflictError(`Session ${sessionId} already has an active run`);
    }

    const result = await this.sessionStore.compactSession(sessionId, keepLastMessages);
    if (!result) {
      throw new SessionNotFoundError(`Session ${sessionId} not found`);
    }

    return {
      checkpoint: result,
      session: await this.getSessionSnapshot(sessionId),
    };
  }
}
