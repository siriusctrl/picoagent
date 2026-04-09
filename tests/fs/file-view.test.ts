import { test } from 'node:test';
import assert from 'node:assert/strict';
import { grepTextBlobs } from '../../src/fs/file-view.js';

test('grepTextBlobs includes surrounding context lines when requested', () => {
  const matches = grepTextBlobs(
    [
      {
        path: 'src/http/server.ts',
        content: ['before', 'needle here', 'after', 'unrelated'].join('\n'),
      },
    ],
    'needle',
    10,
    1,
  );

  assert.deepEqual(matches, [
    { path: 'src/http/server.ts', line: 1, text: 'before', kind: 'context' },
    { path: 'src/http/server.ts', line: 2, text: 'needle here', kind: 'match' },
    { path: 'src/http/server.ts', line: 3, text: 'after', kind: 'context' },
  ]);
});
