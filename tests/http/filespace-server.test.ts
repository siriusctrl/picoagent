import assert from 'node:assert/strict';
import type http from 'node:http';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, test } from 'node:test';
import { HttpFilesystem } from '../../src/fs/http-filesystem.js';
import { startFilespaceServer } from '../../src/http/filespace-server.js';

const servers = new Set<http.Server>();
const roots = new Set<string>();

afterEach(async () => {
  await Promise.all(
    Array.from(servers, (server) => new Promise<void>((resolve, reject) => {
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolve();
      });
    })),
  );
  servers.clear();

  for (const root of roots) {
    rmSync(root, { recursive: true, force: true });
  }
  roots.clear();
});

function serverBaseUrl(server: http.Server): string {
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Expected an inet server address');
  }

  return `http://127.0.0.1:${address.port}`;
}

test('filespace server exposes rooted filesystem operations over HTTP', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-filespace-'));
  roots.add(root);
  writeFileSync(join(root, 'notes.txt'), 'needle line\nsecond line', 'utf8');

  const server = await startFilespaceServer({
    name: 'build',
    root,
    hostname: '127.0.0.1',
    port: 0,
  });
  servers.add(server);

  const filesystem = new HttpFilesystem(serverBaseUrl(server));

  assert.deepEqual(await filesystem.getInfo(), {
    name: 'build',
    writable: true,
    root,
  });
  assert.equal(await filesystem.readTextFile('notes.txt'), 'needle line\nsecond line');
  assert.deepEqual(await filesystem.listFiles('.', 20, new AbortController().signal), ['notes.txt']);
  assert.deepEqual(await filesystem.searchText('.', 'needle', 20, new AbortController().signal), [
    {
      path: 'notes.txt',
      line: 1,
      text: 'needle line',
    },
  ]);

  await filesystem.writeTextFile('created.txt', 'created remotely');
  assert.equal(await filesystem.readTextFile('created.txt'), 'created remotely');

  await filesystem.deleteTextFile('created.txt');
  await assert.rejects(async () => {
    await filesystem.readTextFile('created.txt');
  }, /ENOENT|no such file/i);
});
