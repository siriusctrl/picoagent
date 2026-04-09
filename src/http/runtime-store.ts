import { PicoConfig } from '../config/config.js';
import { AgentPresetId, Message } from '../core/types.js';

export type RunStatus = 'running' | 'completed' | 'failed';

export type RunEvent =
  | {
      type: 'run_started';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      agent: AgentPresetId;
      prompt: string;
    }
  | {
      type: 'assistant_delta';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      text: string;
    }
  | {
      type: 'tool_call';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      title: string;
      toolCallId: string;
      status: 'pending';
      kind?: string;
      rawInput?: unknown;
    }
  | {
      type: 'tool_call_update';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      toolCallId: string;
      title?: string;
      status?: string;
      rawOutput?: unknown;
      text?: string;
    }
  | {
      type: 'done';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      output: string;
    }
  | {
      type: 'error';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      message: string;
    };

export type PendingRunEvent = RunEvent extends infer Event
  ? Event extends { index: number }
    ? Omit<Event, 'index'>
    : never
  : never;

export type EmittedRunEvent = PendingRunEvent extends infer Event
  ? Event extends { timestamp: string }
    ? Omit<Event, 'timestamp'>
    : never
  : never;

export interface RunRecord {
  id: string;
  sessionId?: string;
  agent: AgentPresetId;
  prompt: string;
  status: RunStatus;
  output: string;
  error?: string;
  createdAt: string;
  startedAt?: string;
  finishedAt?: string;
  events: RunEvent[];
}

export interface SessionRecord {
  id: string;
  cwd: string;
  roots: string[];
  agent: AgentPresetId;
  controlVersion: string;
  controlConfig: PicoConfig;
  systemPrompts: Record<AgentPresetId, string>;
  createdAt: string;
  activeRunId?: string;
  runIds: string[];
  messages: Message[];
}

export interface RunSnapshot {
  id: string;
  sessionId?: string;
  agent: AgentPresetId;
  status: RunStatus;
  prompt: string;
  output: string;
  error?: string;
  createdAt: string;
  startedAt?: string;
  finishedAt?: string;
}

export interface SessionSnapshot {
  id: string;
  cwd: string;
  agent: AgentPresetId;
  controlVersion: string;
  controlConfig: Pick<PicoConfig, 'provider' | 'model' | 'maxTokens' | 'contextWindow' | 'baseURL'>;
  createdAt: string;
  activeRunId?: string;
  runs: RunSnapshot[];
}

type RunListener = (event: RunEvent) => void;

export class InMemoryRuntimeStore {
  private readonly sessions = new Map<string, SessionRecord>();
  private readonly runs = new Map<string, RunRecord>();
  private readonly runListeners = new Map<string, Set<RunListener>>();

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
    session.runIds.push(runId);
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
}

export function projectRunSnapshot(run: RunRecord): RunSnapshot {
  return {
    id: run.id,
    sessionId: run.sessionId,
    agent: run.agent,
    status: run.status,
    prompt: run.prompt,
    output: run.output,
    error: run.error,
    createdAt: run.createdAt,
    startedAt: run.startedAt,
    finishedAt: run.finishedAt,
  };
}

export function projectSessionSnapshot(
  session: SessionRecord,
  runs: ReadonlyMap<string, RunRecord>,
): SessionSnapshot {
  return {
    id: session.id,
    cwd: session.cwd,
    agent: session.agent,
    controlVersion: session.controlVersion,
    controlConfig: {
      provider: session.controlConfig.provider,
      model: session.controlConfig.model,
      maxTokens: session.controlConfig.maxTokens,
      contextWindow: session.controlConfig.contextWindow,
      baseURL: session.controlConfig.baseURL,
    },
    createdAt: session.createdAt,
    activeRunId: session.activeRunId,
    runs: session.runIds
      .map((runId) => runs.get(runId))
      .filter((run): run is RunRecord => run !== undefined)
      .map((run) => projectRunSnapshot(run)),
  };
}
