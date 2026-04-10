import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, test } from 'node:test';
import { RootedFilesystem } from '../../src/fs/rooted-fs.js';
import { LocalWorkspaceFileSystem } from '../../src/fs/workspace-fs.js';

const roots = new Set<string>();

afterEach(() => {
  for (const root of roots) {
    rmSync(root, { recursive: true, force: true });
  }
  roots.clear();
});

test('rooted filesystem keeps reads, lists, and searches relative to its root', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-rooted-fs-'));
  roots.add(root);
  mkdirSync(join(root, 'nested'), { recursive: true });
  writeFileSync(join(root, 'nested', 'file.txt'), 'needle here\nand here', 'utf8');

  const filesystem = new RootedFilesystem(new LocalWorkspaceFileSystem(), root);
  const signal = new AbortController().signal;

  assert.equal(await filesystem.readTextFile('nested/file.txt'), 'needle here\nand here');
  assert.deepEqual(await filesystem.listFiles('.', 20, signal), ['nested/file.txt']);
  assert.deepEqual(await filesystem.searchText('.', 'needle', 20, signal), [
    {
      path: 'nested/file.txt',
      line: 1,
      text: 'needle here',
    },
  ]);
});

test('rooted filesystem rejects paths outside the configured root', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-rooted-fs-'));
  roots.add(root);

  const filesystem = new RootedFilesystem(new LocalWorkspaceFileSystem(), root);

  await assert.rejects(async () => {
    await filesystem.readTextFile('../outside.txt');
  }, /outside the rooted filesystem/);
});
