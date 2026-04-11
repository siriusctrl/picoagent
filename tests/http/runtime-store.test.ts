import { expect, test } from 'bun:test';
import { FileRuntimeStore, InMemoryRuntimeStore } from '../../src/runtime/runtime-store.ts';
import { makeTempDir, removeDir } from '../helpers/fs.ts';

function requireValue<T>(value: T | undefined, message: string): T {
  if (value === undefined) {
    throw new Error(message);
  }

  return value;
}

test('runtime store projects ordered session and run snapshots', async () => {
  const store = new InMemoryRuntimeStore();

  await store.createSession({
    id: 'session-1',
    cwd: '/workspace',
    roots: ['/workspace'],
    createdAt: '2025-01-01T00:00:00.000Z',
    runIds: [],
    messages: [],
    checkpoints: [],
  });

  await store.createRun({
    id: 'run-1',
    sessionId: 'session-1',
    prompt: 'first',
    status: 'completed',
    output: 'done first',
    createdAt: '2025-01-01T00:00:01.000Z',
    finishedAt: '2025-01-01T00:00:02.000Z',
    events: [],
  });

  await store.createRun({
    id: 'run-2',
    sessionId: 'session-1',
    prompt: 'second',
    status: 'running',
    output: '',
    createdAt: '2025-01-01T00:00:03.000Z',
    events: [],
  });

  await store.attachRunToSession('session-1', 'run-1');
  await store.clearSessionActiveRun('session-1', 'run-1');
  await store.attachRunToSession('session-1', 'run-2');

  const session = requireValue(store.getSessionSnapshot('session-1'), 'session snapshot should exist');
  expect(session.activeRunId).toBe('run-2');
  expect(session.checkpointCount).toBe(0);
  expect(session.runs.map((run) => [run.id, run.status])).toEqual([
    ['run-1', 'completed'],
    ['run-2', 'running'],
  ]);
});

test('runtime store replays historical run events before streaming new ones', async () => {
  const store = new InMemoryRuntimeStore();
  await store.createRun({
    id: 'run-1',
    prompt: 'hello',
    status: 'running',
    output: '',
    createdAt: '2025-01-01T00:00:00.000Z',
    events: [],
  });

  await store.appendRunEvent('run-1', {
    type: 'run_started',
    timestamp: '2025-01-01T00:00:00.000Z',
    runId: 'run-1',
    prompt: 'hello',
  });

  const seen: string[] = [];
  const unsubscribe = requireValue(store.subscribeToRun('run-1', (event) => {
    seen.push(event.type);
  }), 'subscription should exist');

  await store.appendRunEvent('run-1', {
    type: 'assistant_delta',
    timestamp: '2025-01-01T00:00:01.000Z',
    runId: 'run-1',
    text: 'received',
  });

  unsubscribe();

  await store.appendRunEvent('run-1', {
    type: 'done',
    timestamp: '2025-01-01T00:00:02.000Z',
    runId: 'run-1',
    output: 'received',
  });

  expect(seen).toEqual(['run_started', 'assistant_delta']);
});

test('runtime store clears active session runs and persists conversation on completion', async () => {
  const store = new InMemoryRuntimeStore();

  await store.createSession({
    id: 'session-1',
    cwd: '/workspace',
    roots: ['/workspace'],
    createdAt: '2025-01-01T00:00:00.000Z',
    runIds: [],
    messages: [],
    checkpoints: [],
  });

  await store.attachRunToSession('session-1', 'run-1');
  await store.finishSessionRun('session-1', 'run-1', [
    { role: 'user', content: 'hello' },
    { role: 'assistant', content: [{ type: 'text', text: 'received: hello' }] },
  ]);

  const session = requireValue(store.getSession('session-1'), 'session should exist');
  expect(session.activeRunId).toBeUndefined();
  expect(session.messages).toHaveLength(2);
});

test('runtime store compacts session messages into checkpoints and exposes session resources', async () => {
  const store = new InMemoryRuntimeStore();

  await store.createSession({
    id: 'session-1',
    cwd: '/workspace',
    roots: ['/workspace'],
    createdAt: '2025-01-01T00:00:00.000Z',
    runIds: ['run-1'],
    messages: [
      { role: 'user', content: 'first question' },
      { role: 'assistant', content: [{ type: 'text', text: 'first answer' }] },
      { role: 'user', content: 'second question' },
      { role: 'assistant', content: [{ type: 'text', text: 'second answer' }] },
    ],
    checkpoints: [],
  });

  await store.createRun({
    id: 'run-1',
    sessionId: 'session-1',
    prompt: 'second question',
    status: 'completed',
    output: 'second answer',
    createdAt: '2025-01-01T00:00:01.000Z',
    finishedAt: '2025-01-01T00:00:02.000Z',
    events: [],
  });

  const compacted = requireValue(await store.compactSession('session-1', 2), 'compaction should succeed');
  expect(compacted.checkpointId).not.toHaveLength(0);
  expect(compacted.compactedMessages).toBe(2);
  expect(compacted.keptMessages).toBe(2);

  const session = requireValue(store.getSession('session-1'), 'session should exist');
  expect(session.checkpoints).toHaveLength(1);
  expect(session.activeCheckpointId).toBe(session.checkpoints[0]?.id);
  expect(session.messages).toHaveLength(3);

  expect(store.listSessionResources('session-1', '.')).toEqual(['summary.md', 'checkpoints/', 'runs/', 'events/']);
  expect(store.listSessionResources('session-1', 'checkpoints')).toEqual([`${session.checkpoints[0]?.id}.md`]);
  expect(store.readSessionResource('session-1', 'summary.md') ?? '').toMatch(/# Checkpoint/);
  expect(store.readSessionResource('session-1', 'runs/run-1.md') ?? '').toMatch(/# Run run-1/);
  expect(store.listSessionResources('session-1', 'events')?.[0]).toBe('run-1.jsonl');
});

test('file runtime store reloads sessions, runs, checkpoints, and event logs from disk', async () => {
  const runtimeRoot = await makeTempDir('picoagent-runtime-store-');

  try {
    {
      const store = await FileRuntimeStore.create(runtimeRoot);
      await store.createSession({
        id: 'session-1',
        cwd: '/workspace',
        roots: ['/workspace'],
        createdAt: '2025-01-01T00:00:00.000Z',
        runIds: [],
        messages: [],
        checkpoints: [],
      });

      await store.createRun({
        id: 'run-1',
        sessionId: 'session-1',
        prompt: 'persist me',
        status: 'running',
        output: '',
        createdAt: '2025-01-01T00:00:01.000Z',
        events: [],
      });
      await store.attachRunToSession('session-1', 'run-1');
      await store.appendRunEvent('run-1', {
        type: 'run_started',
        timestamp: '2025-01-01T00:00:01.000Z',
        runId: 'run-1',
        sessionId: 'session-1',
        prompt: 'persist me',
      });
      await store.finishSessionRun('session-1', 'run-1', [
        { role: 'user', content: 'persist me' },
        { role: 'assistant', content: [{ type: 'text', text: 'persisted' }] },
      ]);
      await store.updateRun('run-1', {
        status: 'completed',
        output: 'persisted',
        finishedAt: '2025-01-01T00:00:02.000Z',
      });
      await store.appendRunEvent('run-1', {
        type: 'done',
        timestamp: '2025-01-01T00:00:02.000Z',
        runId: 'run-1',
        sessionId: 'session-1',
        output: 'persisted',
      });
      await store.compactSession('session-1', 1);
    }

    const reloaded = await FileRuntimeStore.create(runtimeRoot);
    const session = requireValue(reloaded.getSessionSnapshot('session-1'), 'reloaded session should exist');
    expect(session.checkpointCount).toBe(1);

    const run = requireValue(reloaded.getRunSnapshot('run-1'), 'reloaded run should exist');
    expect(run.status).toBe('completed');
    expect(run.output).toBe('persisted');

    const events = requireValue(reloaded.getRunEvents('run-1'), 'reloaded events should exist');
    expect(events.events.at(-1)?.type).toBe('done');
    expect(reloaded.readSessionResource('session-1', 'summary.md') ?? '').toMatch(/# Checkpoint/);
    expect(reloaded.readSessionResource('session-1', 'events/run-1.jsonl') ?? '').toMatch(/"type":"run_started"/);
  } finally {
    await removeDir(runtimeRoot);
  }
});
