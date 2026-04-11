import type { Message } from '../core/types.ts';
import { dirnamePath, joinPath, relativePath } from '../fs/path.ts';
import type {
  PendingRunEvent,
  RunEvent,
  RunListener,
  RunRecord,
  RunSnapshot,
  RunStatus,
  RuntimeStore,
  SessionCompactResult,
  SessionRecord,
  SessionSnapshot,
} from './store.ts';
import {
  compactSessionRecord,
  listSessionResourceEntries,
  projectRunSnapshot,
  projectSessionSnapshot,
  readSessionResourceContent,
} from './store-helpers.ts';

interface PersistedRunRecord extends Omit<RunRecord, 'events'> {}

export class InMemoryRuntimeStore implements RuntimeStore {
  protected readonly sessions = new Map<string, SessionRecord>();
  protected readonly runs = new Map<string, RunRecord>();
  protected readonly runListeners = new Map<string, Set<RunListener>>();

  async createSession(record: SessionRecord): Promise<SessionRecord> {
    this.sessions.set(record.id, record);
    return record;
  }

  getSession(id: string): SessionRecord | undefined {
    return this.sessions.get(id);
  }

  async attachRunToSession(sessionId: string, runId: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    session.activeRunId = runId;
    if (!session.runIds.includes(runId)) {
      session.runIds.push(runId);
    }
  }

  async finishSessionRun(sessionId: string, runId: string, messages: Message[]): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    session.messages = messages;
    if (session.activeRunId === runId) {
      session.activeRunId = undefined;
    }
  }

  async clearSessionActiveRun(sessionId: string, runId: string): Promise<void> {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    if (session.activeRunId === runId) {
      session.activeRunId = undefined;
    }
  }

  async createRun(record: RunRecord): Promise<RunRecord> {
    this.runs.set(record.id, record);
    return record;
  }

  getRun(id: string): RunRecord | undefined {
    return this.runs.get(id);
  }

  async updateRun(
    runId: string,
    patch: Partial<Omit<RunRecord, 'id' | 'events'>>,
  ): Promise<RunRecord | undefined> {
    const run = this.runs.get(runId);
    if (!run) {
      return undefined;
    }

    Object.assign(run, patch);
    return run;
  }

  async appendRunEvent(runId: string, event: PendingRunEvent): Promise<RunEvent | undefined> {
    const run = this.runs.get(runId);
    if (!run) {
      return undefined;
    }

    const record = {
      ...event,
      index: run.events.length,
    } as RunEvent;
    run.events.push(record);

    for (const listener of this.runListeners.get(runId) ?? []) {
      listener(record);
    }

    return record;
  }

  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } | undefined {
    const run = this.runs.get(runId);
    if (!run) {
      return undefined;
    }

    return {
      runId,
      status: run.status,
      events: [...run.events],
    };
  }

  subscribeToRun(runId: string, listener: RunListener): (() => void) | undefined {
    const run = this.runs.get(runId);
    if (!run) {
      return undefined;
    }

    let listeners = this.runListeners.get(runId);
    if (!listeners) {
      listeners = new Set();
      this.runListeners.set(runId, listeners);
    }

    listeners.add(listener);
    for (const event of run.events) {
      listener(event);
    }

    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) {
        this.runListeners.delete(runId);
      }
    };
  }

  getRunSnapshot(runId: string): RunSnapshot | undefined {
    const run = this.runs.get(runId);
    if (!run) {
      return undefined;
    }

    return projectRunSnapshot(run);
  }

  getSessionSnapshot(sessionId: string): SessionSnapshot | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    return projectSessionSnapshot(session, this.runs);
  }

  listSessionResources(sessionId: string, resourcePath = '.'): string[] | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    return listSessionResourceEntries(session, resourcePath);
  }

  readSessionResource(sessionId: string, resourcePath: string): string | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    return readSessionResourceContent(session, this.runs, resourcePath);
  }

  async compactSession(sessionId: string, keepLastMessages = 8): Promise<SessionCompactResult | undefined> {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    return compactSessionRecord(sessionId, session, keepLastMessages);
  }
}

export class FileRuntimeStore extends InMemoryRuntimeStore {
  private writeQueue = Promise.resolve();

  private constructor(private readonly runtimeRoot: string) {
    super();
  }

  static async create(runtimeRoot: string): Promise<FileRuntimeStore> {
    const store = new FileRuntimeStore(runtimeRoot);
    await store.loadFromDisk();
    await store.markInterruptedRuns();
    return store;
  }

  override async createSession(record: SessionRecord): Promise<SessionRecord> {
    const session = await super.createSession(record);
    await this.enqueueWrite(() => this.persistSession(session));
    return session;
  }

  override async attachRunToSession(sessionId: string, runId: string): Promise<void> {
    await super.attachRunToSession(sessionId, runId);
    const session = this.getSession(sessionId);
    if (session) {
      await this.enqueueWrite(() => this.persistSession(session));
    }
  }

  override async finishSessionRun(sessionId: string, runId: string, messages: Message[]): Promise<void> {
    await super.finishSessionRun(sessionId, runId, messages);
    const session = this.getSession(sessionId);
    if (session) {
      await this.enqueueWrite(() => this.persistSession(session));
    }
  }

  override async clearSessionActiveRun(sessionId: string, runId: string): Promise<void> {
    await super.clearSessionActiveRun(sessionId, runId);
    const session = this.getSession(sessionId);
    if (session) {
      await this.enqueueWrite(() => this.persistSession(session));
    }
  }

  override async createRun(record: RunRecord): Promise<RunRecord> {
    const run = await super.createRun(record);
    await this.enqueueWrite(() => this.persistRun(run));
    return run;
  }

  override async updateRun(
    runId: string,
    patch: Partial<Omit<RunRecord, 'id' | 'events'>>,
  ): Promise<RunRecord | undefined> {
    const run = await super.updateRun(runId, patch);
    if (run) {
      await this.enqueueWrite(() => this.persistRun(run));
    }
    return run;
  }

  override async appendRunEvent(runId: string, event: PendingRunEvent): Promise<RunEvent | undefined> {
    const record = await super.appendRunEvent(runId, event);
    const run = this.getRun(runId);
    if (record && run) {
      await this.enqueueWrite(async () => {
        await this.appendRunEventRecord(runId, record);
        await this.persistRun(run);
      });
    }
    return record;
  }

  override async compactSession(sessionId: string, keepLastMessages = 8): Promise<SessionCompactResult | undefined> {
    const result = await super.compactSession(sessionId, keepLastMessages);
    const session = this.getSession(sessionId);
    if (result && session) {
      await this.enqueueWrite(() => this.persistSession(session));
    }
    return result;
  }

  private enqueueWrite<T>(task: () => Promise<T>): Promise<T> {
    const next = this.writeQueue.then(task, task);
    this.writeQueue = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }

  private async loadFromDisk(): Promise<void> {
    for (const sessionPath of await scanFiles(this.sessionsDir(), '*/session.json')) {
      const session = await parseJsonFile<SessionRecord>(sessionPath);
      this.sessions.set(session.id, session);
    }

    for (const runPath of await scanFiles(this.runsDir(), '*.json')) {
      const stored = await parseJsonFile<PersistedRunRecord>(runPath);
      const runId = relativePath(this.runsDir(), runPath).slice(0, -'.json'.length);
      const events = await this.loadRunEvents(runId);
      this.runs.set(runId, { ...stored, events });
    }
  }

  private async loadRunEvents(runId: string): Promise<RunEvent[]> {
    const eventsPath = this.runEventsPath(runId);
    const file = Bun.file(eventsPath);
    if (!(await file.exists())) {
      return [];
    }

    const content = (await file.text()).trim();
    if (!content) {
      return [];
    }

    return content
      .split('\n')
      .filter((line) => line.length > 0)
      .map((line) => JSON.parse(line) as RunEvent);
  }

  private async markInterruptedRuns(): Promise<void> {
    const timestamp = new Date().toISOString();

    for (const run of this.runs.values()) {
      if (run.status !== 'running') {
        continue;
      }

      run.status = 'failed';
      run.error = 'Run interrupted by server restart';
      run.finishedAt = timestamp;
      const event = await super.appendRunEvent(run.id, {
        type: 'error',
        timestamp,
        runId: run.id,
        sessionId: run.sessionId,
        message: run.error,
      });

      if (run.sessionId) {
        const session = this.sessions.get(run.sessionId);
        if (session?.activeRunId === run.id) {
          session.activeRunId = undefined;
          await this.persistSession(session);
        }
      }

      if (event) {
        await this.appendRunEventRecord(run.id, event);
      }

      await this.persistRun(run);
    }
  }

  private sessionsDir(): string {
    return joinPath(this.runtimeRoot, 'sessions');
  }

  private runsDir(): string {
    return joinPath(this.runtimeRoot, 'runs');
  }

  private sessionPath(sessionId: string): string {
    return joinPath(this.sessionsDir(), sessionId, 'session.json');
  }

  private runPath(runId: string): string {
    return joinPath(this.runsDir(), `${runId}.json`);
  }

  private runEventsPath(runId: string): string {
    return joinPath(this.runsDir(), `${runId}.events.jsonl`);
  }

  private persistSession(session: SessionRecord): Promise<void> {
    return writeJsonFile(this.sessionPath(session.id), session);
  }

  private persistRun(run: RunRecord): Promise<void> {
    const persisted: PersistedRunRecord = {
      id: run.id,
      sessionId: run.sessionId,
      prompt: run.prompt,
      status: run.status,
      output: run.output,
      error: run.error,
      createdAt: run.createdAt,
      startedAt: run.startedAt,
      finishedAt: run.finishedAt,
    };
    return writeJsonFile(this.runPath(run.id), persisted);
  }

  private async appendRunEventRecord(runId: string, event: RunEvent): Promise<void> {
    const eventsPath = this.runEventsPath(runId);
    const file = Bun.file(eventsPath);
    const existing = await file.exists() ? await file.text() : '';
    await Bun.write(eventsPath, `${existing}${JSON.stringify(event)}\n`);
  }
}

async function scanFiles(root: string, pattern: string): Promise<string[]> {
  try {
    const matches: string[] = [];
    for await (const filePath of new Bun.Glob(pattern).scan({
      cwd: root,
      absolute: true,
      dot: true,
      onlyFiles: true,
      followSymlinks: false,
    })) {
      matches.push(filePath);
    }

    return matches.sort((left, right) => left.localeCompare(right));
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes('ENOENT')) {
      return [];
    }

    throw error;
  }
}

async function parseJsonFile<T>(filePath: string): Promise<T> {
  return JSON.parse(await Bun.file(filePath).text()) as T;
}

async function writeJsonFile(filePath: string, value: unknown): Promise<void> {
  await Bun.write(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

export async function resetFileRuntimeStore(runtimeRoot: string): Promise<void> {
  const process = Bun.spawn(['rm', '-rf', runtimeRoot]);
  const exitCode = await process.exited;
  if (exitCode !== 0) {
    throw new Error(`Failed to reset runtime store at ${runtimeRoot}`);
  }
}
