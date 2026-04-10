import { afterEach, expect, test } from 'bun:test';
import { joinPath } from '../../src/fs/path.ts';
import { RootedFilesystem } from '../../src/fs/rooted-fs.ts';
import { LocalWorkspaceFileSystem } from '../../src/fs/workspace-fs.ts';
import { ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

const roots = new Set<string>();

afterEach(async () => {
  for (const root of roots) {
    await removeDir(root);
  }
  roots.clear();
});

test('rooted filesystem keeps reads, lists, and searches relative to its root', async () => {
  const root = await makeTempDir('picoagent-rooted-fs-');
  roots.add(root);
  await ensureDir(joinPath(root, 'nested'));
  await writeTextFile(joinPath(root, 'nested', 'file.txt'), 'needle here\nand here');

  const filesystem = new RootedFilesystem(new LocalWorkspaceFileSystem(), root);
  const signal = new AbortController().signal;

  expect(await filesystem.readTextFile('nested/file.txt')).toBe('needle here\nand here');
  expect(await filesystem.listFiles('.', 20, signal)).toEqual(['nested/file.txt']);
  expect(await filesystem.searchText('.', 'needle', 20, signal)).toEqual([
    {
      path: 'nested/file.txt',
      line: 1,
      text: 'needle here',
    },
  ]);
});

test('rooted filesystem rejects paths outside the configured root', async () => {
  const root = await makeTempDir('picoagent-rooted-fs-');
  roots.add(root);

  const filesystem = new RootedFilesystem(new LocalWorkspaceFileSystem(), root);

  await expect(filesystem.readTextFile('../outside.txt')).rejects.toThrow(/outside the rooted filesystem/);
});
