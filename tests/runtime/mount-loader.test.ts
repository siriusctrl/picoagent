import { afterEach, expect, test } from 'bun:test';
import type { LocalServerHandle } from '../../src/http/bun-server.ts';
import { startFilespaceServer } from '../../src/http/filespace-server.ts';
import { loadRuntimeMounts } from '../../src/runtime/mount-loader.ts';
import { joinPath } from '../../src/fs/path.ts';
import { makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

const servers = new Set<LocalServerHandle>();
const roots = new Set<string>();

function requireValue<T>(value: T | undefined, message: string): T {
  if (value === undefined) {
    throw new Error(message);
  }

  return value;
}

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

test('loadRuntimeMounts resolves local directory sources into rooted mounts', async () => {
  const root = await makeTempDir('picoagent-local-mount-');
  roots.add(root);
  await writeTextFile(joinPath(root, 'local.txt'), 'local mount data');

  const mounts = await loadRuntimeMounts([{ label: 'local@docs', source: root }], process.cwd());
  expect(mounts).toHaveLength(1);
  const mount = requireValue(mounts[0], 'expected local mount');
  expect(mount.name).toBe('local@docs');
  expect(mount.writable).toBeTruthy();
  expect(mount.executable).toBeFalsy();
  expect(await mount.filesystem.readTextFile('local.txt')).toBe('local mount data');
});

test('loadRuntimeMounts resolves remote filespace urls into http-backed mounts', async () => {
  const root = await makeTempDir('picoagent-remote-mount-');
  roots.add(root);
  await writeTextFile(joinPath(root, 'remote.txt'), 'remote mount data');

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

  expect(mounts).toHaveLength(1);
  const mount = requireValue(mounts[0], 'expected remote mount');
  expect(mount.name).toBe('remote@build');
  expect(mount.writable).toBeTruthy();
  expect(mount.executable).toBeFalsy();
  expect(await mount.filesystem.readTextFile('remote.txt')).toBe('remote mount data');
});

test('loadRuntimeMounts rejects reserved and duplicate labels', async () => {
  await expect(loadRuntimeMounts([{ label: 'workspace', source: '.' }], process.cwd())).rejects.toThrow(/reserved/);

  await expect(
    loadRuntimeMounts(
      [
        { label: 'remote@build', source: '.' },
        { label: 'remote@build', source: './src' },
      ],
      process.cwd(),
    ),
  ).rejects.toThrow(/Duplicate mount label/);
});
