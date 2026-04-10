import assert from 'node:assert/strict';
import { test } from 'node:test';
import { InMemoryRuntimeStore } from '../../src/runtime/runtime-store.js';
import { SessionFilesystem } from '../../src/runtime/session-filesystem.js';
import { StoreBackedSessionStore } from '../../src/runtime/store-backed-session-store.js';

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

test('session filesystem projects summary, checkpoints, and runs as a read-only filesystem', async () => {
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

  store.compactSession('session-1', 2);

  const filesystem = new SessionFilesystem(new StoreBackedSessionStore(store), 'session-1');
  const signal = new AbortController().signal;
  const files = await filesystem.listFiles('.', 20, signal);

  assert.deepEqual(files, [
    'summary.md',
    `${'checkpoints'}/${store.getSession('session-1')?.checkpoints[0]?.id}.md`,
    'runs/run-1.md',
  ]);
  assert.match(await filesystem.readTextFile('summary.md'), /# Checkpoint/);
  assert.match(await filesystem.readTextFile('runs/run-1.md'), /# Run run-1/);

  const matches = await filesystem.searchText('.', 'second answer', 10, signal);
  assert.equal(matches.length, 1);
  assert.equal(matches[0]?.path, 'runs/run-1.md');
  assert.equal(matches[0]?.text, 'second answer');
});
