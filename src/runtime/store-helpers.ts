import type { Message } from '../core/types.ts';
import type {
  RunRecord,
  RunSnapshot,
  SessionCheckpointRecord,
  SessionCompactResult,
  SessionRecord,
  SessionSnapshot,
} from './store.ts';

export function projectRunSnapshot(run: RunRecord): RunSnapshot {
  return {
    id: run.id,
    sessionId: run.sessionId,
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

export function listSessionResourceEntries(session: SessionRecord, resourcePath = '.'): string[] | undefined {
  const normalized = normalizeResourcePath(resourcePath);
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
    return session.runIds.map((runId) => `${runId}.jsonl`);
  }

  return undefined;
}

export function readSessionResourceContent(
  session: SessionRecord,
  runs: ReadonlyMap<string, RunRecord>,
  resourcePath: string,
): string | undefined {
  const normalized = normalizeResourcePath(resourcePath);
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
    const run = runs.get(runMatch[1]);
    return run ? formatRun(run) : undefined;
  }

  const eventsMatch = normalized.match(/^events\/([^/]+)\.jsonl$/);
  if (eventsMatch) {
    const run = runs.get(eventsMatch[1]);
    return run ? run.events.map((event) => JSON.stringify(event)).join('\n') : undefined;
  }

  return undefined;
}

export function compactSessionRecord(
  sessionId: string,
  session: SessionRecord,
  keepLastMessages = 8,
): SessionCompactResult {
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
    id: crypto.randomUUID(),
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

function normalizeResourcePath(value: string): string {
  return value.replace(/^\/+|\/+$/g, '');
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
