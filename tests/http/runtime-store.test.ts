import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { FileRuntimeStore, InMemoryRuntimeStore } from '../../src/runtime/runtime-store.js';

const controlConfig = {
  provider: 'echo' as const,
  model: 'echo',
  maxTokens: 4096,
  contextWindow: 200000,
  baseURL: undefined,
};

const systemPrompts = {
  ask: 'ask prompt',
  exec: 'exec prompt',
};

test('runtime store projects ordered session and run snapshots', () => {
  const store = new InMemoryRuntimeStore();

  store.createSession({
    id: 'session-1',
    cwd: '/workspace',
    roots: ['/workspace'],
    agent: 'ask',
    controlVersion: 'v1',
    controlConfig,
    systemPrompts,
    createdAt: '2025-01-01T00:00:00.000Z',
    runIds: [],
    messages: [],
    checkpoints: [],
  });

  store.createRun({
    id: 'run-1',
    sessionId: 'session-1',
    agent: 'ask',
    prompt: 'first',
    status: 'completed',
    output: 'done first',
    createdAt: '2025-01-01T00:00:01.000Z',
    finishedAt: '2025-01-01T00:00:02.000Z',
    events: [],
  });

  store.createRun({
    id: 'run-2',
    sessionId: 'session-1',
    agent: 'exec',
    prompt: 'second',
    status: 'running',
    output: '',
    createdAt: '2025-01-01T00:00:03.000Z',
    events: [],
  });

  store.attachRunToSession('session-1', 'run-1');
  store.clearSessionActiveRun('session-1', 'run-1');
  store.attachRunToSession('session-1', 'run-2');

  const session = store.getSessionSnapshot('session-1');
  assert.ok(session);
  assert.equal(session.activeRunId, 'run-2');
  assert.equal(session.controlVersion, 'v1');
  assert.equal(session.controlConfig.provider, 'echo');
  assert.equal(session.checkpointCount, 0);
  assert.deepEqual(
    session.runs.map((run) => [run.id, run.agent, run.status]),
    [
      ['run-1', 'ask', 'completed'],
      ['run-2', 'exec', 'running'],
    ],
  );
});

test('runtime store replays historical run events before streaming new ones', () => {
  const store = new InMemoryRuntimeStore();
  store.createRun({
    id: 'run-1',
    agent: 'ask',
    prompt: 'hello',
    status: 'running',
    output: '',
    createdAt: '2025-01-01T00:00:00.000Z',
    events: [],
  });

  store.appendRunEvent('run-1', {
    type: 'run_started',
    timestamp: '2025-01-01T00:00:00.000Z',
    runId: 'run-1',
    agent: 'ask',
    prompt: 'hello',
  });

  const seen: string[] = [];
  const unsubscribe = store.subscribeToRun('run-1', (event) => {
    seen.push(event.type);
  });
  assert.ok(unsubscribe);

  store.appendRunEvent('run-1', {
    type: 'assistant_delta',
    timestamp: '2025-01-01T00:00:01.000Z',
    runId: 'run-1',
    text: 'received',
  });

  unsubscribe();

  store.appendRunEvent('run-1', {
    type: 'done',
    timestamp: '2025-01-01T00:00:02.000Z',
    runId: 'run-1',
    output: 'received',
  });

  assert.deepEqual(seen, ['run_started', 'assistant_delta']);
});

test('runtime store clears active session runs and persists conversation on completion', () => {
  const store = new InMemoryRuntimeStore();

  store.createSession({
    id: 'session-1',
    cwd: '/workspace',
    roots: ['/workspace'],
    agent: 'exec',
    controlVersion: 'v1',
    controlConfig,
    systemPrompts,
    createdAt: '2025-01-01T00:00:00.000Z',
    runIds: [],
    messages: [],
    checkpoints: [],
  });

  store.attachRunToSession('session-1', 'run-1');
  store.finishSessionRun('session-1', 'run-1', [
    { role: 'user', content: 'hello' },
    { role: 'assistant', content: [{ type: 'text', text: 'received: hello' }] },
  ]);

  const session = store.getSession('session-1');
  assert.ok(session);
  assert.equal(session.activeRunId, undefined);
  assert.equal(session.messages.length, 2);
});

test('runtime store compacts session messages into checkpoints and exposes session resources', () => {
  const store = new InMemoryRuntimeStore();

  store.createSession({
    id: 'session-1',
    cwd: '/workspace',
    roots: ['/workspace'],
    agent: 'exec',
    controlVersion: 'v1',
    controlConfig,
    systemPrompts,
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

  store.createRun({
    id: 'run-1',
    sessionId: 'session-1',
    agent: 'exec',
    prompt: 'second question',
    status: 'completed',
    output: 'second answer',
    createdAt: '2025-01-01T00:00:01.000Z',
    finishedAt: '2025-01-01T00:00:02.000Z',
    events: [],
  });

  const compacted = store.compactSession('session-1', 2);
  assert.ok(compacted);
  assert.equal(compacted.checkpointId.length > 0, true);
  assert.equal(compacted.compactedMessages, 2);
  assert.equal(compacted.keptMessages, 2);

  const session = store.getSession('session-1');
  assert.ok(session);
  assert.equal(session.checkpoints.length, 1);
  assert.equal(session.activeCheckpointId, session.checkpoints[0]?.id);
  assert.equal(session.messages.length, 3);

  assert.deepEqual(store.listSessionResources('session-1', '.'), ['summary.md', 'checkpoints/', 'runs/', 'events/']);
  assert.deepEqual(store.listSessionResources('session-1', 'checkpoints'), [`${session.checkpoints[0]?.id}.md`]);
  assert.match(store.readSessionResource('session-1', 'summary.md') ?? '', /# Checkpoint/);
  assert.match(store.readSessionResource('session-1', 'runs/run-1.md') ?? '', /# Run run-1/);
  assert.equal(store.listSessionResources('session-1', 'events')?.[0], 'run-1.jsonl');
});

test('file runtime store reloads sessions, runs, checkpoints, and event logs from disk', () => {
  const runtimeRoot = mkdtempSync(join(tmpdir(), 'picoagent-runtime-store-'));

  try {
    {
      const store = new FileRuntimeStore(runtimeRoot);
      store.createSession({
        id: 'session-1',
        cwd: '/workspace',
        roots: ['/workspace'],
        agent: 'exec',
        controlVersion: 'v1',
        controlConfig,
        systemPrompts,
        createdAt: '2025-01-01T00:00:00.000Z',
        runIds: [],
        messages: [],
        checkpoints: [],
      });

      store.createRun({
        id: 'run-1',
        sessionId: 'session-1',
        agent: 'exec',
        prompt: 'persist me',
        status: 'running',
        output: '',
        createdAt: '2025-01-01T00:00:01.000Z',
        events: [],
      });
      store.attachRunToSession('session-1', 'run-1');
      store.appendRunEvent('run-1', {
        type: 'run_started',
        timestamp: '2025-01-01T00:00:01.000Z',
        runId: 'run-1',
        sessionId: 'session-1',
        agent: 'exec',
        prompt: 'persist me',
      });
      store.finishSessionRun('session-1', 'run-1', [
        { role: 'user', content: 'persist me' },
        { role: 'assistant', content: [{ type: 'text', text: 'persisted' }] },
      ]);
      store.updateRun('run-1', {
        status: 'completed',
        output: 'persisted',
        finishedAt: '2025-01-01T00:00:02.000Z',
      });
      store.appendRunEvent('run-1', {
        type: 'done',
        timestamp: '2025-01-01T00:00:02.000Z',
        runId: 'run-1',
        sessionId: 'session-1',
        output: 'persisted',
      });
      store.compactSession('session-1', 1);
    }

    const reloaded = new FileRuntimeStore(runtimeRoot);
    const session = reloaded.getSessionSnapshot('session-1');
    assert.ok(session);
    assert.equal(session.checkpointCount, 1);

    const run = reloaded.getRunSnapshot('run-1');
    assert.ok(run);
    assert.equal(run.status, 'completed');
    assert.equal(run.output, 'persisted');

    const events = reloaded.getRunEvents('run-1');
    assert.ok(events);
    assert.equal(events.events.at(-1)?.type, 'done');
    assert.match(reloaded.readSessionResource('session-1', 'summary.md') ?? '', /# Checkpoint/);
    assert.match(reloaded.readSessionResource('session-1', 'events/run-1.jsonl') ?? '', /"type":"run_started"/);
  } finally {
    rmSync(runtimeRoot, { recursive: true, force: true });
  }
});
