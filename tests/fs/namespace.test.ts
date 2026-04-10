import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Namespace } from '../../src/fs/namespace.js';
import type { MutableFilesystem } from '../../src/core/filesystem.js';

test('namespace preserves absolute paths for mounted filesystem access', async () => {
  const calls: string[] = [];
  const filesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      calls.push(`read:${filePath}`);
      return 'ok';
    },
    async writeTextFile(filePath, content) {
      calls.push(`write:${filePath}:${content}`);
    },
    async deleteTextFile(filePath) {
      calls.push(`delete:${filePath}`);
    },
    async listFiles(root) {
      calls.push(`list:${root}`);
      return [`${root}/a.ts`];
    },
    async searchText(root, query) {
      calls.push(`search:${root}:${query}`);
      return [{ path: `${root}/a.ts`, line: 1, text: 'needle' }];
    },
  };

  const namespace = new Namespace([{ name: 'workspace', filesystem, root: '.', writable: true }]);
  const absolutePath = '/tmp/project/src/a.ts';

  assert.equal(await namespace.readTextFile('workspace', absolutePath), 'ok');
  await namespace.writeTextFile('workspace', absolutePath, 'updated');
  await namespace.deleteTextFile('workspace', absolutePath);
  assert.deepEqual(await namespace.listFiles('workspace', absolutePath, 10, new AbortController().signal), [
    '/tmp/project/src/a.ts/a.ts',
  ]);
  assert.deepEqual(await namespace.searchText('workspace', absolutePath, 'needle', 10, new AbortController().signal), [
    { path: '/tmp/project/src/a.ts/a.ts', line: 1, text: 'needle' },
  ]);

  assert.deepEqual(calls, [
    'read:/tmp/project/src/a.ts',
    'write:/tmp/project/src/a.ts:updated',
    'delete:/tmp/project/src/a.ts',
    'list:/tmp/project/src/a.ts',
    'search:/tmp/project/src/a.ts:needle',
  ]);
});
