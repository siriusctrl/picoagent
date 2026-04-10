import assert from 'node:assert/strict';
import type http from 'node:http';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, test } from 'node:test';
import { startFilespaceServer } from '../../src/http/filespace-server.js';
import { loadRuntimeMounts } from '../../src/runtime/mount-loader.js';

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

test('loadRuntimeMounts resolves local directory sources into rooted mounts', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-local-mount-'));
  roots.add(root);
  writeFileSync(join(root, 'local.txt'), 'local mount data', 'utf8');

  const mounts = await loadRuntimeMounts([{ label: 'local@docs', source: root }], process.cwd());
  assert.equal(mounts.length, 1);
  assert.equal(mounts[0]?.name, 'local@docs');
  assert.equal(mounts[0]?.writable, true);
  assert.equal(mounts[0]?.executable, false);
  assert.equal(await mounts[0]!.filesystem.readTextFile('local.txt'), 'local mount data');
});

test('loadRuntimeMounts resolves remote filespace urls into http-backed mounts', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-remote-mount-'));
  roots.add(root);
  writeFileSync(join(root, 'remote.txt'), 'remote mount data', 'utf8');

  const server = await startFilespaceServer({
    name: 'build',
    root,
    hostname: '127.0.0.1',
    port: 0,
  });
  servers.add(server);

  const mounts = await loadRuntimeMounts(
    [{ label: 'remote@build', source: serverBaseUrl(server) }],
    process.cwd(),
  );

  assert.equal(mounts.length, 1);
  assert.equal(mounts[0]?.name, 'remote@build');
  assert.equal(mounts[0]?.writable, true);
  assert.equal(mounts[0]?.executable, false);
  assert.equal(await mounts[0]!.filesystem.readTextFile('remote.txt'), 'remote mount data');
});

test('loadRuntimeMounts rejects reserved and duplicate labels', async () => {
  await assert.rejects(async () => {
    await loadRuntimeMounts([{ label: 'workspace', source: '.' }], process.cwd());
  }, /reserved/);

  await assert.rejects(async () => {
    await loadRuntimeMounts(
      [
        { label: 'remote@build', source: '.' },
        { label: 'remote@build', source: './src' },
      ],
      process.cwd(),
    );
  }, /Duplicate mount label/);
});
