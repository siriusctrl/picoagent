import { expect, test } from 'bun:test';
import { searchFiles, walkFiles } from '../../src/fs/filesystem.ts';
import { joinPath } from '../../src/fs/path.ts';
import { basename, ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

test('searchFiles scans past the initial file window before declaring no matches', async () => {
  const root = await makeTempDir('picoagent-search-');

  try {
    for (let index = 0; index < 260; index += 1) {
      const fileName = `file-${String(index).padStart(3, '0')}.txt`;
      await writeTextFile(joinPath(root, fileName), 'no match here\n');
    }

    await writeTextFile(joinPath(root, 'zz-target.txt'), 'Needle in the workspace\n');

    const matches = await searchFiles(root, 'needle', 50, new AbortController().signal);

    expect(matches).toHaveLength(1);
    expect(basename(matches[0].path)).toBe('zz-target.txt');
    expect(matches[0].line).toBe(1);
  } finally {
    await removeDir(root);
  }
});

test('walkFiles includes hidden files and skips ignored directories', async () => {
  const root = await makeTempDir('picoagent-walk-');

  try {
    await Promise.all([
      ensureDir(joinPath(root, '.hidden')),
      ensureDir(joinPath(root, '.git')),
      ensureDir(joinPath(root, 'node_modules', 'pkg')),
      ensureDir(joinPath(root, 'dist')),
      ensureDir(joinPath(root, 'src')),
    ]);

    await Promise.all([
      writeTextFile(joinPath(root, '.env'), 'SECRET=1\n'),
      writeTextFile(joinPath(root, '.hidden', 'keep.txt'), 'keep\n'),
      writeTextFile(joinPath(root, '.git', 'config'), 'skip\n'),
      writeTextFile(joinPath(root, 'node_modules', 'pkg', 'index.js'), 'skip\n'),
      writeTextFile(joinPath(root, 'dist', 'bundle.js'), 'skip\n'),
      writeTextFile(joinPath(root, 'src', 'app.ts'), 'keep\n'),
    ]);

    const files = await walkFiles(root, 20, new AbortController().signal);

    expect(files.map((filePath) => filePath.slice(root.length + 1))).toEqual([
      '.env',
      '.hidden/keep.txt',
      'src/app.ts',
    ]);
  } finally {
    await removeDir(root);
  }
});
