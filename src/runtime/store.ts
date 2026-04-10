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
  activeCheckpointId?: string;
  runIds: string[];
  messages: Message[];
  checkpoints: SessionCheckpointRecord[];
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

export interface RuntimeStore {
  createSession(record: SessionRecord): SessionRecord;
  getSession(id: string): SessionRecord | undefined;
  setSessionAgent(sessionId: string, agent: AgentPresetId): void;
  refreshSessionControl(
    sessionId: string,
    control: {
      controlVersion: string;
      controlConfig: PicoConfig;
      systemPrompts: Record<AgentPresetId, string>;
    },
  ): void;
  attachRunToSession(sessionId: string, runId: string): void;
  finishSessionRun(sessionId: string, runId: string, messages: Message[]): void;
  clearSessionActiveRun(sessionId: string, runId: string): void;
  createRun(record: RunRecord): RunRecord;
  getRun(id: string): RunRecord | undefined;
  updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): RunRecord | undefined;
  appendRunEvent(runId: string, event: PendingRunEvent): RunEvent | undefined;
  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } | undefined;
  subscribeToRun(runId: string, listener: RunListener): (() => void) | undefined;
  getRunSnapshot(runId: string): RunSnapshot | undefined;
  getSessionSnapshot(sessionId: string): SessionSnapshot | undefined;
  listSessionResources(sessionId: string, resourcePath?: string): string[] | undefined;
  readSessionResource(sessionId: string, resourcePath: string): string | undefined;
  compactSession(sessionId: string, keepLastMessages?: number): SessionCompactResult | undefined;
}

export type RunListener = (event: RunEvent) => void;
