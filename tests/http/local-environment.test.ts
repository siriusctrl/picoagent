import assert from 'node:assert/strict';
import { test } from 'node:test';
import { LocalEnvironment } from '../../src/http/local-environment.js';
import { WorkspaceFileSystem } from '../../src/fs/workspace-fs.js';

test('local environment delegates file reads and writes to the workspace filesystem', async () => {
  const calls: string[] = [];
  const fileSystem: WorkspaceFileSystem = {
    async readTextFile(filePath, options) {
      calls.push(`read:${filePath}:${options?.line ?? 0}:${options?.limit ?? 0}`);
      return 'hello';
    },
    async writeTextFile(filePath, content) {
      calls.push(`write:${filePath}:${content}`);
    },
    async listFiles() {
      return [];
    },
    async searchText() {
      return [];
    },
  };

  const environment = new LocalEnvironment(fileSystem);
  assert.equal(await environment.readTextFile('session-1', '/workspace/a.ts', { line: 2, limit: 3 }), 'hello');
  await environment.writeTextFile('session-1', '/workspace/a.ts', 'updated');

  assert.deepEqual(calls, [
    'read:/workspace/a.ts:2:3',
    'write:/workspace/a.ts:updated',
  ]);
});

test('local environment delegates listing and text search to the workspace filesystem', async () => {
  const signal = new AbortController().signal;
  const fileSystem: WorkspaceFileSystem = {
    async readTextFile() {
      return '';
    },
    async writeTextFile() {},
    async listFiles(root, limit, receivedSignal) {
      assert.equal(root, '/workspace');
      assert.equal(limit, 2);
      assert.equal(receivedSignal, signal);
      return ['/workspace/a.ts', '/workspace/b.ts'];
    },
    async searchText(root, query, limit, receivedSignal) {
      assert.equal(root, '/workspace');
      assert.equal(query, 'needle');
      assert.equal(limit, 1);
      assert.equal(receivedSignal, signal);
      return [{ path: '/workspace/a.ts', line: 4, text: 'needle' }];
    },
  };

  const environment = new LocalEnvironment(fileSystem);

  assert.deepEqual(await environment.listFiles('/workspace', 2, signal), ['/workspace/a.ts', '/workspace/b.ts']);
  assert.deepEqual(await environment.searchText('/workspace', 'needle', 1, signal), [
    { path: '/workspace/a.ts', line: 4, text: 'needle' },
  ]);
});
