import { expect, test } from 'bun:test';
import { filterGlob, grepTextBlobs } from '../../src/fs/file-view.ts';

test('filterGlob uses Bun glob semantics for brace patterns when available', () => {
  const matches = filterGlob(
    [
      'src/http/server.ts',
      'src/runtime/service.ts',
      'src/http/openapi.ts',
    ],
    'src/{http,runtime}/*.ts',
  );

  expect(matches).toEqual([
    'src/http/openapi.ts',
    'src/http/server.ts',
    'src/runtime/service.ts',
  ]);
});

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

  expect(matches).toEqual([
    { path: 'src/http/server.ts', line: 1, text: 'before', kind: 'context' },
    { path: 'src/http/server.ts', line: 2, text: 'needle here', kind: 'match' },
    { path: 'src/http/server.ts', line: 3, text: 'after', kind: 'context' },
  ]);
});
