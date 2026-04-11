import { Message } from '../core/types.ts';

export type RunStatus = 'running' | 'completed' | 'failed';

export type RunEvent =
  | {
      type: 'run_started';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
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
  createdAt: string;
  activeRunId?: string;
  activeCheckpointId?: string;
  runIds: string[];
  messages: Message[];
  checkpoints: SessionCheckpointRecord[];
}

export interface RunSnapshot {
  id: string;
  sessionId?: string;
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
  createdAt: string;
  activeRunId?: string;
  activeCheckpointId?: string;
  checkpointCount: number;
  runs: RunSnapshot[];
}

export interface SessionCheckpointRecord {
  id: string;
  sessionId: string;
  parentCheckpointId?: string;
  createdAt: string;
  compactedMessages: number;
  keptMessages: number;
  summary: string;
}

export interface SessionCompactResult {
  checkpointId: string;
  summary: string;
  compactedMessages: number;
  keptMessages: number;
}

export interface RunStore {
  createRun(record: RunRecord): Promise<RunRecord>;
  getRun(id: string): RunRecord | undefined;
  updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): Promise<RunRecord | undefined>;
  appendRunEvent(runId: string, event: PendingRunEvent): Promise<RunEvent | undefined>;
  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } | undefined;
  subscribeToRun(runId: string, listener: RunListener): (() => void) | undefined;
  getRunSnapshot(runId: string): RunSnapshot | undefined;
}

export interface SessionStore {
  createSession(record: SessionRecord): Promise<SessionRecord>;
  getSession(id: string): Promise<SessionRecord | undefined>;
  createRun(record: RunRecord): Promise<void>;
  updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): Promise<void>;
  appendRunEvent(runId: string, event: PendingRunEvent): Promise<void>;
  attachRunToSession(sessionId: string, runId: string): Promise<void>;
  finishSessionRun(sessionId: string, runId: string, messages: Message[]): Promise<void>;
  clearSessionActiveRun(sessionId: string, runId: string): Promise<void>;
  getSessionSnapshot(sessionId: string): Promise<SessionSnapshot | undefined>;
  listSessionResources(sessionId: string, resourcePath?: string): Promise<string[] | undefined>;
  readSessionResource(sessionId: string, resourcePath: string): Promise<string | undefined>;
  compactSession(sessionId: string, keepLastMessages?: number): Promise<SessionCompactResult | undefined>;
}

export interface RuntimeStore {
  createSession(record: SessionRecord): Promise<SessionRecord>;
  getSession(id: string): SessionRecord | undefined;
  attachRunToSession(sessionId: string, runId: string): Promise<void>;
  finishSessionRun(sessionId: string, runId: string, messages: Message[]): Promise<void>;
  clearSessionActiveRun(sessionId: string, runId: string): Promise<void>;
  createRun(record: RunRecord): Promise<RunRecord>;
  getRun(id: string): RunRecord | undefined;
  updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): Promise<RunRecord | undefined>;
  appendRunEvent(runId: string, event: PendingRunEvent): Promise<RunEvent | undefined>;
  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } | undefined;
  subscribeToRun(runId: string, listener: RunListener): (() => void) | undefined;
  getRunSnapshot(runId: string): RunSnapshot | undefined;
  getSessionSnapshot(sessionId: string): SessionSnapshot | undefined;
  listSessionResources(sessionId: string, resourcePath?: string): string[] | undefined;
  readSessionResource(sessionId: string, resourcePath: string): string | undefined;
  compactSession(sessionId: string, keepLastMessages?: number): Promise<SessionCompactResult | undefined>;
}

export type RunListener = (event: RunEvent) => void;
