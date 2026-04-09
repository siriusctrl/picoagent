import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ToolContext } from '../../src/core/types.js';
import { cmdTool } from '../../src/tools/cmd.js';
import { globTool } from '../../src/tools/glob.js';
import { grepTool } from '../../src/tools/grep.js';
import { patchTool } from '../../src/tools/patch.js';
import { readTool } from '../../src/tools/read.js';

const context: ToolContext = {
  runId: 'run-1',
  sessionId: 'session-1',
  cwd: '/workspace',
  roots: ['/workspace'],
  controlRoot: '/workspace',
  agent: 'exec',
  signal: new AbortController().signal,
  fileView: {
    glob: async (target, pattern) =>
      target === 'session' ? ['summary.md', 'runs/run-1.md'] : ['src/http/server.ts', pattern],
    grep: async (target, query) => [
      {
        path: target === 'session' ? 'runs/run-1.md' : '/workspace/src/http/server.ts',
        line: 12,
        text: `${query} here`,
      },
    ],
    read: async (target, path) => `${target}:${path}`,
    patch: async () => [
      {
        path: '/workspace/src/http/server.ts',
        action: 'update',
        oldText: 'old',
        newText: 'new',
      },
      {
        path: '/workspace/src/http/routes.ts',
        action: 'create',
        newText: 'created',
      },
    ],
    cmd: async (_target, request) => ({
      terminalId: 'term-1',
      output: [request.command, ...(request.args ?? [])].join(' '),
      truncated: false,
      exitCode: 0,
      signal: null,
    }),
  },
};

test('glob lists matching paths from a target file-view', async () => {
  const result = await globTool.execute({ target: 'session', pattern: '**/*.md' }, context);
  assert.equal(result.content, 'summary.md\nruns/run-1.md');
});

test('grep searches a target file-view', async () => {
  const result = await grepTool.execute({ target: 'workspace', query: 'needle' }, context);
  assert.equal(result.content, 'src/http/server.ts:12: needle here');
});

test('read reads one target-relative path', async () => {
  const result = await readTool.execute({ target: 'session', path: 'summary.md' }, context);
  assert.match(result.content, /session:summary.md/);
});

test('patch applies multi-file changes through the file-view', async () => {
  const result = await patchTool.execute(
    {
      target: 'workspace',
      operations: [
        {
          type: 'replace',
          path: 'src/http/server.ts',
          oldText: 'old',
          newText: 'new',
        },
        {
          type: 'create',
          path: 'src/http/routes.ts',
          content: 'created',
        },
      ],
    },
    context,
  );

  assert.equal(result.display?.length, 2);
  assert.equal(result.locations?.length, 2);
});

test('cmd executes against an executable target', async () => {
  const result = await cmdTool.execute({ target: 'workspace', command: 'npm test' }, context);
  assert.match(result.content, /bash -lc npm test/);
  assert.equal(result.display?.[0]?.type, 'terminal');
});
