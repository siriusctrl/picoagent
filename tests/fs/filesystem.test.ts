import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { tmpdir } from 'node:os';
import { searchFiles } from '../../src/fs/filesystem.js';

test('searchFiles scans past the initial file window before declaring no matches', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-search-'));

  try {
    for (let index = 0; index < 260; index += 1) {
      const fileName = `file-${String(index).padStart(3, '0')}.txt`;
      writeFileSync(join(root, fileName), 'no match here\n', 'utf8');
    }

    writeFileSync(join(root, 'zz-target.txt'), 'Needle in the workspace\n', 'utf8');

    const matches = await searchFiles(root, 'needle', 50, new AbortController().signal);

    assert.equal(matches.length, 1);
    assert.equal(basename(matches[0].path), 'zz-target.txt');
    assert.equal(matches[0].line, 1);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
