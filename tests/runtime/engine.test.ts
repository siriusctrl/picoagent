import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createRuntimeContext } from '../../src/runtime/index.js';
import { RuntimeConflictError, RuntimeEngine } from '../../src/runtime/engine.js';
import { InMemoryRuntimeStore } from '../../src/runtime/runtime-store.js';
import { LocalWorkspaceFileSystem } from '../../src/fs/workspace-fs.js';
import type { MutableFilesystem } from '../../src/core/filesystem.js';

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

test('runtime engine rejects a second concurrent session run after control refresh awaits', async () => {
  const baseFilesystem = new LocalWorkspaceFileSystem();
  const delayedFilesystem: MutableFilesystem = {
    async readTextFile(filePath, options) {
      await delay(20);
      return baseFilesystem.readTextFile(filePath, options);
    },
    async writeTextFile(filePath, content) {
      await baseFilesystem.writeTextFile(filePath, content);
    },
    async deleteTextFile(filePath) {
      await baseFilesystem.deleteTextFile(filePath);
    },
    async listFiles(root, limit, signal) {
      await delay(20);
      return baseFilesystem.listFiles(root, limit, signal);
    },
    async searchText(root, query, limit, signal) {
      await delay(20);
      return baseFilesystem.searchText(root, query, limit, signal);
    },
  };

  const store = new InMemoryRuntimeStore();
  const engine = new RuntimeEngine({
    cwd: process.cwd(),
    filesystem: delayedFilesystem,
    executionBackend: {
      async run() {
        throw new Error('Execution should not run in this test');
      },
    },
    runtimeContext: createRuntimeContext(process.cwd()),
    store,
  });

  (engine as unknown as { startRun: (run: unknown) => void }).startRun = () => {};

  const session = await engine.createSession('ask');
  const results = await Promise.allSettled([
    engine.createSessionRun(session, 'first concurrent turn'),
    engine.createSessionRun(session, 'second concurrent turn'),
  ]);

  const fulfilled = results.filter(
    (result): result is PromiseFulfilledResult<Awaited<ReturnType<RuntimeEngine['createSessionRun']>>> => result.status === 'fulfilled',
  );
  const rejected = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected');

  assert.equal(fulfilled.length, 1);
  assert.equal(rejected.length, 1);
  assert.ok(rejected[0].reason instanceof RuntimeConflictError);

  const storedSession = store.getSession(session.id);
  assert.equal(storedSession?.activeRunId, fulfilled[0].value.id);
  assert.deepEqual(storedSession?.runIds, [fulfilled[0].value.id]);
});
