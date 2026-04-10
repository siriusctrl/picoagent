import { randomUUID } from 'node:crypto';
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import type { PicoConfig } from '../config/config.js';
import type { AgentPresetId, Message } from '../core/types.js';
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
} from './store.js';
import {
  compactSessionRecord,
  listSessionResourceEntries,
  projectRunSnapshot,
  projectSessionSnapshot,
  readSessionResourceContent,
} from './store-helpers.js';

interface PersistedRunRecord extends Omit<RunRecord, 'events'> {}

export class InMemoryRuntimeStore implements RuntimeStore {
  protected readonly sessions = new Map<string, SessionRecord>();
  protected readonly runs = new Map<string, RunRecord>();
  protected readonly runListeners = new Map<string, Set<RunListener>>();

  createSession(record: SessionRecord): SessionRecord {
    this.sessions.set(record.id, record);
    return record;
  }

  getSession(id: string): SessionRecord | undefined {
    return this.sessions.get(id);
  }

  setSessionAgent(sessionId: string, agent: AgentPresetId): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    session.agent = agent;
  }

  refreshSessionControl(
    sessionId: string,
    control: {
      controlVersion: string;
      controlConfig: PicoConfig;
      systemPrompts: Record<AgentPresetId, string>;
    },
  ): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    session.controlVersion = control.controlVersion;
    session.controlConfig = control.controlConfig;
    session.systemPrompts = control.systemPrompts;
  }

  attachRunToSession(sessionId: string, runId: string): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    session.activeRunId = runId;
    if (!session.runIds.includes(runId)) {
      session.runIds.push(runId);
    }
  }

  finishSessionRun(sessionId: string, runId: string, messages: Message[]): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    session.messages = messages;
    if (session.activeRunId === runId) {
      session.activeRunId = undefined;
    }
  }

  clearSessionActiveRun(sessionId: string, runId: string): void {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return;
    }

    if (session.activeRunId === runId) {
      session.activeRunId = undefined;
    }
  }

  createRun(record: RunRecord): RunRecord {
    this.runs.set(record.id, record);
    return record;
  }

  getRun(id: string): RunRecord | undefined {
    return this.runs.get(id);
  }

  updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): RunRecord | undefined {
    const run = this.runs.get(runId);
    if (!run) {
      return undefined;
    }

    Object.assign(run, patch);
    return run;
  }

  appendRunEvent(runId: string, event: PendingRunEvent): RunEvent | undefined {
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

  compactSession(sessionId: string, keepLastMessages = 8): SessionCompactResult | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    return compactSessionRecord(sessionId, session, keepLastMessages);
  }
}

export class FileRuntimeStore extends InMemoryRuntimeStore {
  constructor(private readonly runtimeRoot: string) {
    super();
    mkdirSync(this.sessionsDir(), { recursive: true });
    mkdirSync(this.runsDir(), { recursive: true });
    this.loadFromDisk();
    this.markInterruptedRuns();
  }

  override createSession(record: SessionRecord): SessionRecord {
    const session = super.createSession(record);
    this.persistSession(session);
    return session;
  }

  override setSessionAgent(sessionId: string, agent: AgentPresetId): void {
    super.setSessionAgent(sessionId, agent);
    const session = this.getSession(sessionId);
    if (session) {
      this.persistSession(session);
    }
  }

  override refreshSessionControl(
    sessionId: string,
    control: {
      controlVersion: string;
      controlConfig: PicoConfig;
      systemPrompts: Record<AgentPresetId, string>;
    },
  ): void {
    super.refreshSessionControl(sessionId, control);
    const session = this.getSession(sessionId);
    if (session) {
      this.persistSession(session);
    }
  }

  override attachRunToSession(sessionId: string, runId: string): void {
    super.attachRunToSession(sessionId, runId);
    const session = this.getSession(sessionId);
    if (session) {
      this.persistSession(session);
    }
  }

  override finishSessionRun(sessionId: string, runId: string, messages: Message[]): void {
    super.finishSessionRun(sessionId, runId, messages);
    const session = this.getSession(sessionId);
    if (session) {
      this.persistSession(session);
    }
  }

  override clearSessionActiveRun(sessionId: string, runId: string): void {
    super.clearSessionActiveRun(sessionId, runId);
    const session = this.getSession(sessionId);
    if (session) {
      this.persistSession(session);
    }
  }

  override createRun(record: RunRecord): RunRecord {
    const run = super.createRun(record);
    this.persistRun(run);
    return run;
  }

  override updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): RunRecord | undefined {
    const run = super.updateRun(runId, patch);
    if (run) {
      this.persistRun(run);
    }
    return run;
  }

  override appendRunEvent(runId: string, event: PendingRunEvent): RunEvent | undefined {
    const record = super.appendRunEvent(runId, event);
    const run = this.getRun(runId);
    if (record && run) {
      this.appendRunEventRecord(runId, record);
      this.persistRun(run);
    }
    return record;
  }

  override compactSession(sessionId: string, keepLastMessages = 8): SessionCompactResult | undefined {
    const result = super.compactSession(sessionId, keepLastMessages);
    const session = this.getSession(sessionId);
    if (result && session) {
      this.persistSession(session);
    }
    return result;
  }

  private loadFromDisk(): void {
    for (const entry of readdirSync(this.sessionsDir(), { withFileTypes: true })) {
      if (!entry.isDirectory()) {
        continue;
      }

      const sessionPath = join(this.sessionsDir(), entry.name, 'session.json');
      if (!existsSync(sessionPath)) {
        continue;
      }

      const session = parseJsonFile<SessionRecord>(sessionPath);
      this.sessions.set(session.id, session);
    }

    for (const entry of readdirSync(this.runsDir(), { withFileTypes: true })) {
      if (!entry.isFile() || !entry.name.endsWith('.json') || entry.name.endsWith('.events.jsonl')) {
        continue;
      }

      const runPath = join(this.runsDir(), entry.name);
      const stored = parseJsonFile<PersistedRunRecord>(runPath);
      const runId = entry.name.slice(0, -'.json'.length);
      const events = this.loadRunEvents(runId);
      this.runs.set(runId, { ...stored, events });
    }
  }

  private loadRunEvents(runId: string): RunEvent[] {
    const eventsPath = this.runEventsPath(runId);
    if (!existsSync(eventsPath)) {
      return [];
    }

    const content = readFileSync(eventsPath, 'utf8').trim();
    if (!content) {
      return [];
    }

    return content
      .split('\n')
      .filter((line) => line.length > 0)
      .map((line) => JSON.parse(line) as RunEvent);
  }

  private markInterruptedRuns(): void {
    const timestamp = new Date().toISOString();

    for (const run of this.runs.values()) {
      if (run.status !== 'running') {
        continue;
      }

      run.status = 'failed';
      run.error = 'Run interrupted by server restart';
      run.finishedAt = timestamp;
      const event = super.appendRunEvent(run.id, {
        type: 'error',
        timestamp,
        runId: run.id,
        sessionId: run.sessionId,
        message: run.error,
      });
      if (event) {
        this.appendRunEventRecord(run.id, event);
      }

      if (run.sessionId) {
        const session = this.sessions.get(run.sessionId);
        if (session?.activeRunId === run.id) {
          session.activeRunId = undefined;
          this.persistSession(session);
        }
      }

      this.persistRun(run);
    }
  }

  private sessionsDir(): string {
    return join(this.runtimeRoot, 'sessions');
  }

  private runsDir(): string {
    return join(this.runtimeRoot, 'runs');
  }

  private sessionPath(sessionId: string): string {
    return join(this.sessionsDir(), sessionId, 'session.json');
  }

  private runPath(runId: string): string {
    return join(this.runsDir(), `${runId}.json`);
  }

  private runEventsPath(runId: string): string {
    return join(this.runsDir(), `${runId}.events.jsonl`);
  }

  private persistSession(session: SessionRecord): void {
    writeJsonFileAtomic(this.sessionPath(session.id), session);
  }

  private persistRun(run: RunRecord): void {
    const persisted: PersistedRunRecord = {
      id: run.id,
      sessionId: run.sessionId,
      agent: run.agent,
      prompt: run.prompt,
      status: run.status,
      output: run.output,
      error: run.error,
      createdAt: run.createdAt,
      startedAt: run.startedAt,
      finishedAt: run.finishedAt,
    };
    writeJsonFileAtomic(this.runPath(run.id), persisted);
  }

  private appendRunEventRecord(runId: string, event: RunEvent): void {
    const eventsPath = this.runEventsPath(runId);
    mkdirSync(dirname(eventsPath), { recursive: true });
    appendFileSync(eventsPath, `${JSON.stringify(event)}\n`, 'utf8');
  }
}

function parseJsonFile<T>(path: string): T {
  return JSON.parse(readFileSync(path, 'utf8')) as T;
}

function writeJsonFileAtomic(path: string, value: unknown): void {
  mkdirSync(dirname(path), { recursive: true });
  const tempPath = join(dirname(path), `${randomUUID()}.tmp`);
  writeFileSync(tempPath, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
  renameSync(tempPath, path);
}

export function resetFileRuntimeStore(runtimeRoot: string): void {
  rmSync(runtimeRoot, { recursive: true, force: true });
}
