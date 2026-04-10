import { expect, test } from 'bun:test';
import { ToolContext } from '../../src/core/types.ts';
import { cmdTool } from '../../src/tools/cmd.ts';
import { globTool } from '../../src/tools/glob.ts';
import { grepTool } from '../../src/tools/grep.ts';
import { patchTool } from '../../src/tools/patch.ts';
import { readTool } from '../../src/tools/read.ts';

const context: ToolContext = {
  runId: 'run-1',
  sessionId: 'session-1',
  cwd: '/workspace',
  roots: ['/workspace'],
  controlRoot: '/workspace',
  agent: 'exec',
  signal: new AbortController().signal,
  fileView: {
    glob: async (pattern) =>
      pattern.startsWith('/session') ? ['/session/summary.md', '/session/runs/run-1.md'] : ['/workspace/src/http/server.ts'],
    grep: async (query) => [
      {
        path: '/workspace/src/http/server.ts',
        line: 12,
        text: `${query} here`,
      },
    ],
    read: async (path) => path,
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
    cmd: async (request) => ({
      terminalId: 'term-1',
      output: [request.command, ...(request.args ?? [])].join(' '),
      truncated: false,
      exitCode: 0,
      signal: null,
    }),
  },
};

test('glob lists matching paths from a target file-view', async () => {
  const result = await globTool.execute({ pattern: '/session/**/*.md' }, context);
  expect(result.content).toBe('/session/summary.md\n/session/runs/run-1.md');
});

test('grep searches a target file-view', async () => {
  const result = await grepTool.execute({ path: '/workspace', query: 'needle' }, context);
  expect(result.content).toBe('/workspace/src/http/server.ts:12: needle here');
});

test('grep renders surrounding context lines distinctly', async () => {
  const contextWithSurrounding: ToolContext = {
    ...context,
    fileView: {
      ...context.fileView,
      grep: async () => [
        { path: '/workspace/src/http/server.ts', line: 11, text: 'before', kind: 'context' },
        { path: '/workspace/src/http/server.ts', line: 12, text: 'needle here', kind: 'match' },
        { path: '/workspace/src/http/server.ts', line: 13, text: 'after', kind: 'context' },
      ],
    },
  };

  const result = await grepTool.execute({ path: '/workspace', query: 'needle', context: 1 }, contextWithSurrounding);

  expect(result.content).toBe(
    '/workspace/src/http/server.ts-11- before\n/workspace/src/http/server.ts:12: needle here\n/workspace/src/http/server.ts-13- after',
  );
  expect(result.locations).toEqual([{ path: '/workspace/src/http/server.ts', line: 12 }]);
});

test('read reads one target-relative path', async () => {
  const result = await readTool.execute({ path: '/session/summary.md' }, context);
  expect(result.content).toMatch(/\/session\/summary.md/);
});

test('patch applies multi-file changes through the file-view', async () => {
  const result = await patchTool.execute(
    {
      operations: [
        {
          type: 'replace',
          path: '/workspace/src/http/server.ts',
          oldText: 'old',
          newText: 'new',
        },
        {
          type: 'create',
          path: '/workspace/src/http/routes.ts',
          content: 'created',
        },
      ],
    },
    context,
  );

  expect(result.display).toHaveLength(2);
  expect(result.locations).toHaveLength(2);
});

test('cmd executes against an executable target', async () => {
  const result = await cmdTool.execute({ command: 'npm test' }, context);
  expect(result.content).toMatch(/bash -lc npm test/);
  expect(result.display?.[0]?.type).toBe('terminal');
});
