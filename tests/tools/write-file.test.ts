import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { writeFileTool } from '../../src/tools/write-file.js';
import { ToolContext } from '../../src/core/types.js';
import { searchFiles, walkFiles } from '../../src/fs/filesystem.js';

function createContext(root: string): ToolContext {
  return {
    sessionId: 'session-1',
    cwd: root,
    roots: [root],
    controlRoot: root,
    agent: 'exec',
    signal: new AbortController().signal,
    environment: {
      readTextFile: async (_sessionId, filePath) => readFileSync(filePath, 'utf8'),
      writeTextFile: async (_sessionId, filePath, content) => {
        await import('node:fs/promises').then(({ mkdir, writeFile }) =>
          mkdir(dirname(filePath), { recursive: true }).then(() => writeFile(filePath, content, 'utf8')),
        );
      },
      listFiles: (dir, limit, signal) => walkFiles(dir, limit, signal),
      searchText: (dir, query, limit, signal) => searchFiles(dir, query, limit, signal),
      runCommand: async () => ({
        terminalId: 'term-1',
        output: '',
        truncated: false,
        exitCode: 0,
        signal: null,
      }),
    },
  };
}

test('write_file writes inside the session root and returns a diff payload', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-write-'));
  const context = createContext(root);

  const result = await writeFileTool.execute({ path: 'notes/todo.txt', content: 'hello' }, context);
  assert.equal(result.content, 'Created notes/todo.txt');
  assert.equal(result.display?.[0]?.type, 'diff');
  assert.equal(readFileSync(join(root, 'notes', 'todo.txt'), 'utf8'), 'hello');

  rmSync(root, { recursive: true, force: true });
});

test('write_file rejects paths outside the session roots', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-write-'));
  const context = createContext(root);

  await assert.rejects(() => writeFileTool.execute({ path: '../escape.txt', content: 'nope' }, context));
  rmSync(root, { recursive: true, force: true });
});
