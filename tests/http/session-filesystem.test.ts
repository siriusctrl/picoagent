import { expect, test } from 'bun:test';
import { InMemoryRuntimeStore } from '../../src/runtime/runtime-store.ts';
import { SessionFilesystem } from '../../src/runtime/session-filesystem.ts';
import { StoreBackedSessionStore } from '../../src/runtime/store-backed-session-store.ts';

test('session filesystem projects summary, checkpoints, and runs as a read-only filesystem', async () => {
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

  await store.compactSession('session-1', 2);

  const filesystem = new SessionFilesystem(new StoreBackedSessionStore(store), 'session-1');
  const signal = new AbortController().signal;
  const files = await filesystem.listFiles('.', 20, signal);

  expect(files).toEqual([
    'summary.md',
    `${'checkpoints'}/${store.getSession('session-1')?.checkpoints[0]?.id}.md`,
    'runs/run-1.md',
  ]);
  expect(await filesystem.readTextFile('summary.md')).toMatch(/# Checkpoint/);
  expect(await filesystem.readTextFile('runs/run-1.md')).toMatch(/# Run run-1/);

  const matches = await filesystem.searchText('.', 'second answer', 10, signal);
  expect(matches).toHaveLength(1);
  expect(matches[0]?.path).toBe('runs/run-1.md');
  expect(matches[0]?.text).toBe('second answer');
});
