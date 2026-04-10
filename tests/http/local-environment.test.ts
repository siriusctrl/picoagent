import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { test } from 'node:test';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { LocalWorkspaceFileSystem, WorkspaceFileSystem } from '../../src/fs/workspace-fs.js';
import { LocalExecutionBackend } from '../../src/runtime/local-execution-backend.js';

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
  assert.equal(await filesystem.readTextFile('/workspace/a.ts', { line: 2, limit: 3 }), 'hello');
  await filesystem.writeTextFile('/workspace/a.ts', 'updated');

  assert.deepEqual(calls, [
    'read:/workspace/a.ts:2:3',
    'write:/workspace/a.ts:updated',
  ]);
});

test('local environment deletes files directly from the filesystem', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-local-env-'));

  try {
    const filePath = join(root, 'delete-me.txt');
    writeFileSync(filePath, 'bye', 'utf8');

    const filesystem = new LocalWorkspaceFileSystem();

    await filesystem.deleteTextFile(filePath);

    assert.throws(() => readFileSync(filePath, 'utf8'));
  } finally {
    rmSync(root, { recursive: true, force: true });
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

  const filesystem = new LocalWorkspaceFileSystem(fileSystem);

  assert.deepEqual(await filesystem.listFiles('/workspace', 2, signal), ['/workspace/a.ts', '/workspace/b.ts']);
  assert.deepEqual(await filesystem.searchText('/workspace', 'needle', 1, signal), [
    { path: '/workspace/a.ts', line: 4, text: 'needle' },
  ]);
});

test('local execution backend runs a command and returns terminal metadata', async () => {
  const execution = new LocalExecutionBackend();
  const result = await execution.run({
    runId: 'test-run',
    command: 'node',
    args: ['-e', 'console.log("hello")'],
    outputByteLimit: 64000,
  });

  assert.equal(result.terminalId.startsWith('test-run:'), true);
  assert.match(result.output, /hello/);
  assert.equal(result.exitCode, 0);
  assert.equal(result.signal, null);
});
