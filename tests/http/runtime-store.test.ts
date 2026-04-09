import assert from 'node:assert/strict';
import { test } from 'node:test';
import { InMemoryRuntimeStore } from '../../src/http/runtime-store.js';

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
