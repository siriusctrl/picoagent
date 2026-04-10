import { afterEach, expect, test } from 'bun:test';
import type { MutableFilesystem } from '../../src/core/filesystem.ts';
import { HttpFilesystem } from '../../src/fs/http-filesystem.ts';
import type { LocalServerHandle } from '../../src/http/bun-server.ts';
import { startFilespaceServer } from '../../src/http/filespace-server.ts';
import { joinPath } from '../../src/fs/path.ts';
import { makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

const servers = new Set<LocalServerHandle>();
const roots = new Set<string>();

afterEach(async () => {
  await Promise.all(Array.from(servers, (server) => server.stop(true)));
  servers.clear();

  for (const root of roots) {
    await removeDir(root);
  }
  roots.clear();
});

function serverBaseUrl(server: LocalServerHandle): string {
  return server.url.origin;
}

test('filespace server exposes rooted filesystem operations over HTTP', async () => {
  const root = await makeTempDir('picoagent-filespace-');
  roots.add(root);
  await writeTextFile(joinPath(root, 'notes.txt'), 'needle line\nsecond line');

  const server = await startFilespaceServer({
    name: 'build',
    root,
    hostname: '127.0.0.1',
    port: 0,
  });
  servers.add(server);

  const filesystem = new HttpFilesystem(serverBaseUrl(server));

  expect(await filesystem.getInfo()).toEqual({
    name: 'build',
    writable: true,
    root,
  });
  expect(await filesystem.readTextFile('notes.txt')).toBe('needle line\nsecond line');
  expect(await filesystem.listFiles('.', 20, new AbortController().signal)).toEqual(['notes.txt']);
  expect(await filesystem.searchText('.', 'needle', 20, new AbortController().signal)).toEqual([
    {
      path: 'notes.txt',
      line: 1,
      text: 'needle line',
    },
  ]);

  await filesystem.writeTextFile('created.txt', 'created remotely');
  expect(await filesystem.readTextFile('created.txt')).toBe('created remotely');

  await filesystem.deleteTextFile('created.txt');
  await expect(filesystem.readTextFile('created.txt')).rejects.toThrow(/ENOENT|no such file/i);
});

test('filespace server returns 400 for malformed JSON request bodies', async () => {
  const root = await makeTempDir('picoagent-filespace-');
  roots.add(root);

  const server = await startFilespaceServer({
    name: 'build',
    root,
    hostname: '127.0.0.1',
    port: 0,
  });
  servers.add(server);

  const response = await fetch(`${serverBaseUrl(server)}/read`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"path":',
  });
  expect(response.status).toBe(400);
  expect(await response.json()).toEqual({ error: 'Malformed JSON in request body' });
});

test('filespace server returns 500 for backend filesystem failures', async () => {
  const root = await makeTempDir('picoagent-filespace-');
  roots.add(root);

  const failingFilesystem: MutableFilesystem = {
    async readTextFile() {
      throw new Error('backend exploded');
    },
    async writeTextFile() {},
    async deleteTextFile() {},
    async listFiles() {
      return [];
    },
    async searchText() {
      return [];
    },
  };

  const server = await startFilespaceServer({
    name: 'build',
    root,
    hostname: '127.0.0.1',
    port: 0,
    filesystem: failingFilesystem,
  });
  servers.add(server);

  const response = await fetch(`${serverBaseUrl(server)}/read`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ path: 'notes.txt' }),
  });
  expect(response.status).toBe(500);
  expect(await response.json()).toEqual({ error: 'backend exploded' });
});
