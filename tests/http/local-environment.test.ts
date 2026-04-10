import { expect, test } from 'bun:test';
import { LocalWorkspaceFileSystem, WorkspaceFileSystem } from '../../src/fs/workspace-fs.ts';
import { LocalExecutionBackend } from '../../src/runtime/local-execution-backend.ts';
import { joinPath } from '../../src/fs/path.ts';
import { makeTempDir, readTextFile, removeDir, writeTextFile } from '../helpers/fs.ts';

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
    async deleteTextFile() {},
    async listFiles() {
      return [];
    },
    async searchText() {
      return [];
    },
  };

  const filesystem = new LocalWorkspaceFileSystem(fileSystem);
  expect(await filesystem.readTextFile('/workspace/a.ts', { line: 2, limit: 3 })).toBe('hello');
  await filesystem.writeTextFile('/workspace/a.ts', 'updated');

  expect(calls).toEqual([
    'read:/workspace/a.ts:2:3',
    'write:/workspace/a.ts:updated',
  ]);
});

test('local environment deletes files directly from the filesystem', async () => {
  const root = await makeTempDir('picoagent-local-env-');

  try {
    const filePath = joinPath(root, 'delete-me.txt');
    await writeTextFile(filePath, 'bye');

    const filesystem = new LocalWorkspaceFileSystem();

    await filesystem.deleteTextFile(filePath);

    await expect(readTextFile(filePath)).rejects.toThrow();
  } finally {
    await removeDir(root);
  }
});

test('local environment delegates listing and text search to the workspace filesystem', async () => {
  const signal = new AbortController().signal;
  const fileSystem: WorkspaceFileSystem = {
    async readTextFile() {
      return '';
    },
    async writeTextFile() {},
    async deleteTextFile() {},
    async listFiles(root, limit, receivedSignal) {
      expect(root).toBe('/workspace');
      expect(limit).toBe(2);
      expect(receivedSignal).toBe(signal);
      return ['/workspace/a.ts', '/workspace/b.ts'];
    },
    async searchText(root, query, limit, receivedSignal) {
      expect(root).toBe('/workspace');
      expect(query).toBe('needle');
      expect(limit).toBe(1);
      expect(receivedSignal).toBe(signal);
      return [{ path: '/workspace/a.ts', line: 4, text: 'needle' }];
    },
  };

  const filesystem = new LocalWorkspaceFileSystem(fileSystem);

  expect(await filesystem.listFiles('/workspace', 2, signal)).toEqual([
    '/workspace/a.ts',
    '/workspace/b.ts',
  ]);
  expect(await filesystem.searchText('/workspace', 'needle', 1, signal)).toEqual([
    { path: '/workspace/a.ts', line: 4, text: 'needle' },
  ]);
});

test('local execution backend runs a command and returns terminal metadata', async () => {
  const execution = new LocalExecutionBackend();
  const result = await execution.run({
    runId: 'test-run',
    command: 'bun',
    args: ['-e', 'console.log("hello")'],
    outputByteLimit: 64000,
  });

  expect(result.terminalId).toMatch(/^test-run:/);
  expect(result.output).toMatch(/hello/);
  expect(result.exitCode).toBe(0);
  expect(result.signal).toBe(null);
});

test('local execution backend captures stderr and truncates oversized output', async () => {
  const execution = new LocalExecutionBackend();
  const result = await execution.run({
    runId: 'test-run',
    command: 'bun',
    args: ['-e', 'process.stdout.write("a".repeat(40000)); process.stderr.write("!");'],
    outputByteLimit: 1024,
  });

  expect(result.truncated).toBeTruthy();
  expect(result.output.length).toBeLessThanOrEqual(1024);
  expect(result.output).toContain('!');
  expect(result.exitCode).toBe(0);
});
