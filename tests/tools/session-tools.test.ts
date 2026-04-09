import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ToolContext } from '../../src/core/types.js';
import { compactSessionTool } from '../../src/tools/compact-session.js';
import { listSessionResourcesTool } from '../../src/tools/list-session-resources.js';
import { readSessionResourceTool } from '../../src/tools/read-session-resource.js';

const context: ToolContext = {
  runId: 'run-1',
  sessionId: 'session-1',
  cwd: process.cwd(),
  roots: [process.cwd()],
  controlRoot: process.cwd(),
  agent: 'exec',
  signal: new AbortController().signal,
  environment: {
    readTextFile: async () => '',
    writeTextFile: async () => {},
    listFiles: async () => [],
    searchText: async () => [],
    runCommand: async () => ({
      terminalId: 'term-1',
      output: '',
      truncated: false,
      exitCode: 0,
      signal: null,
    }),
  },
  sessionAccess: {
    listResources: async (_sessionId, path) => (path === 'checkpoints' ? ['cp-1.md'] : ['summary.md', 'runs/']),
    readResource: async (_sessionId, path) => `content for ${path}`,
    compactSession: async () => ({
      checkpointId: 'cp-1',
      summary: 'summary text',
      compactedMessages: 10,
      keptMessages: 4,
    }),
  },
};

test('list_session_resources lists virtual session resources', async () => {
  const result = await listSessionResourcesTool.execute({ path: 'checkpoints' }, context);
  assert.equal(result.content, 'cp-1.md');
});

test('read_session_resource reads one virtual session resource', async () => {
  const result = await readSessionResourceTool.execute({ path: 'summary.md' }, context);
  assert.equal(result.content, 'content for summary.md');
});

test('compact_session creates a checkpoint summary', async () => {
  const result = await compactSessionTool.execute({ keepLastMessages: 4 }, context);
  assert.match(result.content, /Created checkpoint cp-1/);
  assert.deepEqual(result.rawOutput, {
    checkpointId: 'cp-1',
    summary: 'summary text',
    compactedMessages: 10,
    keptMessages: 4,
  });
});

test('session history tools reject standalone runs without a session', async () => {
  const standaloneContext: ToolContext = {
    ...context,
    sessionId: undefined,
  };

  await assert.rejects(
    () => listSessionResourcesTool.execute({}, standaloneContext),
    /persistent session/,
  );
  await assert.rejects(
    () => readSessionResourceTool.execute({ path: 'summary.md' }, standaloneContext),
    /persistent session/,
  );
  await assert.rejects(
    () => compactSessionTool.execute({}, standaloneContext),
    /persistent session/,
  );
});
