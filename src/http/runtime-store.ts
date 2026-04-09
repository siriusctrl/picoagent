import { randomUUID } from 'node:crypto';
import { PicoConfig } from '../config/config.js';
import { AgentPresetId, Message } from '../core/types.js';
import { SessionCompactResult } from '../core/session-access.js';

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

  listSessionResources(sessionId: string, resourcePath = '.'): string[] | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    const normalized = resourcePath.replace(/^\/+|\/+$/g, '');
    if (normalized === '' || normalized === '.') {
      return ['summary.md', 'checkpoints/', 'runs/', 'events/'];
    }

    if (normalized === 'checkpoints') {
      return session.checkpoints.map((checkpoint) => `${checkpoint.id}.md`);
    }

    if (normalized === 'runs') {
      return session.runIds.map((runId) => `${runId}.md`);
    }

    if (normalized === 'events') {
      return session.runIds.map((runId) => `${runId}.ndjson`);
    }

    return undefined;
  }

  readSessionResource(sessionId: string, resourcePath: string): string | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    const normalized = resourcePath.replace(/^\/+/, '');
    if (normalized === 'summary.md') {
      const checkpoint = session.activeCheckpointId
        ? session.checkpoints.find((candidate) => candidate.id === session.activeCheckpointId)
        : undefined;
      if (!checkpoint) {
        return 'No session checkpoint yet.';
      }

      return formatCheckpoint(checkpoint);
    }

    const checkpointMatch = normalized.match(/^checkpoints\/([^/]+)\.md$/);
    if (checkpointMatch) {
      const checkpoint = session.checkpoints.find((candidate) => candidate.id === checkpointMatch[1]);
      return checkpoint ? formatCheckpoint(checkpoint) : undefined;
    }

    const runMatch = normalized.match(/^runs\/([^/]+)\.md$/);
    if (runMatch) {
      const run = this.runs.get(runMatch[1]);
      return run ? formatRun(run) : undefined;
    }

    const eventsMatch = normalized.match(/^events\/([^/]+)\.ndjson$/);
    if (eventsMatch) {
      const run = this.runs.get(eventsMatch[1]);
      return run ? run.events.map((event) => JSON.stringify(event)).join('\n') : undefined;
    }

    return undefined;
  }

  compactSession(sessionId: string, keepLastMessages = 8): SessionCompactResult | undefined {
    const session = this.sessions.get(sessionId);
    if (!session) {
      return undefined;
    }

    const compactedMessages = Math.max(session.messages.length - keepLastMessages, 0);
    if (compactedMessages <= 0) {
      const checkpoint = session.activeCheckpointId
        ? session.checkpoints.find((candidate) => candidate.id === session.activeCheckpointId)
        : undefined;
      return {
        checkpointId: checkpoint?.id ?? '',
        summary: checkpoint?.summary ?? 'Nothing to compact.',
        compactedMessages: 0,
        keptMessages: session.messages.length,
      };
    }

    const compacted = session.messages.slice(0, compactedMessages);
    const tail = session.messages.slice(compactedMessages);
    const summary = summarizeMessages(compacted);
    const checkpoint: SessionCheckpointRecord = {
      id: randomUUID(),
      sessionId,
      parentCheckpointId: session.activeCheckpointId,
      createdAt: new Date().toISOString(),
      compactedMessages,
      keptMessages: tail.length,
      summary,
    };

    session.checkpoints.push(checkpoint);
    session.activeCheckpointId = checkpoint.id;
    session.messages = [
      {
        role: 'assistant',
        content: [{ type: 'text', text: `Session checkpoint ${checkpoint.id}\n\n${summary}` }],
      },
      ...tail,
    ];

    return {
      checkpointId: checkpoint.id,
      summary,
      compactedMessages,
      keptMessages: tail.length,
    };
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
    activeCheckpointId: session.activeCheckpointId,
    checkpointCount: session.checkpoints.length,
    runs: session.runIds
      .map((runId) => runs.get(runId))
      .filter((run): run is RunRecord => run !== undefined)
      .map((run) => projectRunSnapshot(run)),
  };
}

function summarizeMessages(messages: Message[]): string {
  const lines = messages
    .map((message) => {
      if (message.role === 'user') {
        return `user: ${truncateLine(message.content)}`;
      }

      if (message.role === 'assistant') {
        const text = message.content
          .filter((item): item is { type: 'text'; text: string } => item.type === 'text')
          .map((item) => item.text)
          .join(' ');
        const toolCalls = message.content.flatMap((item) =>
          item.type === 'toolCall' ? [item.name] : [],
        );
        const suffix = toolCalls.length > 0 ? ` [tools: ${toolCalls.join(', ')}]` : '';
        return `assistant: ${truncateLine(text || '(tool call response)')}${suffix}`;
      }

      return `tool: ${truncateLine(message.content)}`;
    })
    .slice(-24);

  return lines.length > 0 ? lines.join('\n') : 'No prior conversation.';
}

function truncateLine(value: string, limit = 240): string {
  return value.length > limit ? `${value.slice(0, limit)}...` : value;
}

function formatCheckpoint(checkpoint: SessionCheckpointRecord): string {
  return [
    `# Checkpoint ${checkpoint.id}`,
    `createdAt: ${checkpoint.createdAt}`,
    `parentCheckpointId: ${checkpoint.parentCheckpointId ?? 'none'}`,
    `compactedMessages: ${checkpoint.compactedMessages}`,
    `keptMessages: ${checkpoint.keptMessages}`,
    '',
    checkpoint.summary,
  ].join('\n');
}

function formatRun(run: RunRecord): string {
  return [
    `# Run ${run.id}`,
    `sessionId: ${run.sessionId ?? 'none'}`,
    `agent: ${run.agent}`,
    `status: ${run.status}`,
    `createdAt: ${run.createdAt}`,
    `startedAt: ${run.startedAt ?? 'n/a'}`,
    `finishedAt: ${run.finishedAt ?? 'n/a'}`,
    '',
    '## Prompt',
    run.prompt,
    '',
    '## Output',
    run.output || '(empty)',
    ...(run.error ? ['', '## Error', run.error] : []),
  ].join('\n');
}
