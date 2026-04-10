import { expect, test } from 'bun:test';
import { joinPath } from '../../src/fs/path.ts';
import { createRuntimeContext } from '../../src/runtime/index.ts';
import { RuntimeConflictError, RuntimeEngine } from '../../src/runtime/engine.ts';
import { InMemoryRuntimeStore } from '../../src/runtime/runtime-store.ts';
import { StoreBackedSessionStore } from '../../src/runtime/store-backed-session-store.ts';
import { LocalWorkspaceFileSystem } from '../../src/fs/workspace-fs.ts';
import type { MutableFilesystem } from '../../src/core/filesystem.ts';
import type { FileViewAccess } from '../../src/core/file-view.ts';

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
    runStore: store,
    sessionStore: new StoreBackedSessionStore(store),
    executionBackend: {
      async run() {
        throw new Error('Execution should not run in this test');
      },
    },
    runtimeContext: createRuntimeContext(process.cwd()),
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

  expect(fulfilled).toHaveLength(1);
  expect(rejected).toHaveLength(1);
  expect(rejected[0].reason).toBeInstanceOf(RuntimeConflictError);

  const storedSession = store.getSession(session.id);
  expect(storedSession?.activeRunId).toBe(fulfilled[0].value.id);
  expect(storedSession?.runIds).toEqual([fulfilled[0].value.id]);
});

test('fileView supports namespace paths for workspace read', async () => {
  const basePath = '/tmp/picoagent-workspace';
  const readCalls: string[] = [];
  const workspaceFile = joinPath(basePath, 'workspace-file.txt');
  const filesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      readCalls.push(filePath);
      if (filePath === workspaceFile) {
        return 'workspace-data';
      }
      return 'missing';
    },
    async writeTextFile(filePath, content) {
      throw new Error(`unexpected write ${filePath}: ${content}`);
    },
    async deleteTextFile(filePath) {
      throw new Error(`unexpected delete ${filePath}`);
    },
    async listFiles(root, limit, signal) {
      return [];
    },
    async searchText(root, query, limit, signal) {
      return [];
    },
  };

  const store = new InMemoryRuntimeStore();
  const engine = new RuntimeEngine({
    cwd: basePath,
    filesystem,
    runStore: store,
    sessionStore: new StoreBackedSessionStore(store),
    executionBackend: {
      async run() {
        throw new Error('execution should not run');
      },
    },
    runtimeContext: createRuntimeContext(process.cwd()),
  });

  const runtime = engine as unknown as {
    fileView: (
      runId: string,
      cwd: string,
      roots: string[],
      signal: AbortSignal,
      sessionId?: string,
    ) => Pick<FileViewAccess, 'read'>;
  };
  const methods = runtime.fileView('run-1', basePath, [basePath], new AbortController().signal);
  const content = await methods.read('/workspace/workspace-file.txt');

  expect(content).toBe('workspace-data');
  expect(readCalls).toEqual([workspaceFile]);
});

test('fileView supports session namespace read', async () => {
  const filesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      throw new Error(`unexpected read ${filePath}`);
    },
    async writeTextFile(filePath, content) {
      throw new Error(`unexpected write ${filePath}`);
    },
    async deleteTextFile(filePath) {
      throw new Error(`unexpected delete ${filePath}`);
    },
    async listFiles(root, limit, signal) {
      return [];
    },
    async searchText(root, query, limit, signal) {
      return [];
    },
  };

  const store = new InMemoryRuntimeStore();
  const engine = new RuntimeEngine({
    cwd: process.cwd(),
    filesystem,
    runStore: store,
    sessionStore: new StoreBackedSessionStore(store),
    executionBackend: {
      async run() {
        throw new Error('execution should not run');
      },
    },
    runtimeContext: createRuntimeContext(process.cwd()),
  });

  const session = await engine.createSession('ask');

  const runtime = engine as unknown as {
    fileView: (
      runId: string,
      cwd: string,
      roots: string[],
      signal: AbortSignal,
      sessionId?: string,
    ) => Pick<FileViewAccess, 'read'>;
  };
  const methods = runtime.fileView(
    'run-2',
    process.cwd(),
    [process.cwd()],
    new AbortController().signal,
    session.id,
  );

  const content = await methods.read('/session/summary.md');
  expect(content).toContain('No session checkpoint yet.');
});

test('fileView requires session id for session namespace', async () => {
  const filesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      throw new Error(`unexpected read ${filePath}`);
    },
    async writeTextFile(filePath, content) {
      throw new Error(`unexpected write ${filePath}`);
    },
    async deleteTextFile(filePath) {
      throw new Error(`unexpected delete ${filePath}`);
    },
    async listFiles(root, limit, signal) {
      return [];
    },
    async searchText(root, query, limit, signal) {
      return [];
    },
  };

  const store = new InMemoryRuntimeStore();
  const engine = new RuntimeEngine({
    cwd: process.cwd(),
    filesystem,
    runStore: store,
    sessionStore: new StoreBackedSessionStore(store),
    executionBackend: {
      async run() {
        throw new Error('execution should not run');
      },
    },
    runtimeContext: createRuntimeContext(process.cwd()),
  });

  const runtime = engine as unknown as {
    fileView: (
      runId: string,
      cwd: string,
      roots: string[],
      signal: AbortSignal,
      sessionId?: string,
    ) => Pick<FileViewAccess, 'read'>;
  };
  const methods = runtime.fileView('run-3', process.cwd(), [process.cwd()], new AbortController().signal);

  await expect(methods.read('/session/summary.md')).rejects.toThrow(/session namespace requires a persistent session/);
});

test('fileView preserves extra namespace mounts when a session namespace is active', async () => {
  const remoteFile = '/remote-root/docs/readme.md';
  const filesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      throw new Error(`unexpected workspace read ${filePath}`);
    },
    async writeTextFile(filePath, content) {
      throw new Error(`unexpected workspace write ${filePath}: ${content}`);
    },
    async deleteTextFile(filePath) {
      throw new Error(`unexpected workspace delete ${filePath}`);
    },
    async listFiles(root, limit, signal) {
      return [];
    },
    async searchText(root, query, limit, signal) {
      return [];
    },
  };

  const remoteFilesystem: MutableFilesystem = {
    async readTextFile(filePath) {
      if (filePath === remoteFile) {
        return 'remote mount data';
      }
      throw new Error(`unexpected remote read ${filePath}`);
    },
    async writeTextFile(filePath, content) {
      throw new Error(`unexpected remote write ${filePath}: ${content}`);
    },
    async deleteTextFile(filePath) {
      throw new Error(`unexpected remote delete ${filePath}`);
    },
    async listFiles(root, limit, signal) {
      return [];
    },
    async searchText(root, query, limit, signal) {
      return [];
    },
  };

  const store = new InMemoryRuntimeStore();
  const engine = new RuntimeEngine({
    cwd: process.cwd(),
    filesystem,
    runStore: store,
    sessionStore: new StoreBackedSessionStore(store),
    mounts: [
      {
        name: 'remote@build',
        filesystem: remoteFilesystem,
        root: '/remote-root',
      },
    ],
    executionBackend: {
      async run() {
        throw new Error('execution should not run');
      },
    },
    runtimeContext: createRuntimeContext(process.cwd()),
  });

  const session = await engine.createSession('ask');

  const runtime = engine as unknown as {
    fileView: (
      runId: string,
      cwd: string,
      roots: string[],
      signal: AbortSignal,
      sessionId?: string,
    ) => Pick<FileViewAccess, 'read'>;
  };
  const methods = runtime.fileView(
    'run-4',
    process.cwd(),
    [process.cwd()],
    new AbortController().signal,
    session.id,
  );

  expect(await methods.read('/remote@build/docs/readme.md')).toBe('remote mount data');
});
