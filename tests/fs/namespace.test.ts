import { expect, test } from 'bun:test';
import { Namespace } from '../../src/fs/namespace.ts';
import type { MutableFilesystem } from '../../src/core/filesystem.ts';

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

  expect(await namespace.readTextFile('workspace', absolutePath)).toBe('ok');
  await namespace.writeTextFile('workspace', absolutePath, 'updated');
  await namespace.deleteTextFile('workspace', absolutePath);
  expect(await namespace.listFiles('workspace', absolutePath, 10, new AbortController().signal)).toEqual([
    '/tmp/project/src/a.ts/a.ts',
  ]);
  expect(await namespace.searchText('workspace', absolutePath, 'needle', 10, new AbortController().signal)).toEqual([
    { path: '/tmp/project/src/a.ts/a.ts', line: 1, text: 'needle' },
  ]);

  expect(calls).toEqual([
    'read:/tmp/project/src/a.ts',
    'write:/tmp/project/src/a.ts:updated',
    'delete:/tmp/project/src/a.ts',
    'list:/tmp/project/src/a.ts',
    'search:/tmp/project/src/a.ts:needle',
  ]);
});

test('namespace resolves absolute-like namespace paths', async () => {
  const filesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      return filePath;
    },
    async writeTextFile(filePath, content) {},
    async deleteTextFile(filePath) {},
    async listFiles(root, limit, signal) {
      return [];
    },
    async searchText(root, query, signal, options) {
      return [];
    },
  };

  const namespace = new Namespace([{ name: 'workspace', filesystem, root: '.', writable: true }]);
  const result = namespace.resolveNamespacePath('/workspace/src/main.ts');

  expect(result.mountName).toBe('workspace');
  expect(result.relativePath).toBe('src/main.ts');
});

test('namespace rejects unknown namespace path mount', async () => {
  const filesystem = {
    async readTextFile(filePath: string) {
      return '';
    },
    async writeTextFile(filePath: string, content: string) {},
    async deleteTextFile(filePath: string) {},
    async listFiles(root: string, limit: number, signal: AbortSignal) {
      return [];
    },
    async searchText(root: string, query: string, limit: number, signal: AbortSignal) {
      return [];
    },
  };

  const namespace = new Namespace([{ name: 'workspace', filesystem, root: '.' }]);

  expect(() => {
    namespace.resolveNamespacePath('/ghost/path');
  }).toThrow(/Unknown namespace mount: ghost/);
});
