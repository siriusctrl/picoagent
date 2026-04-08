import { test } from 'node:test';
import assert from 'node:assert/strict';
import { z } from 'zod';
import { runAgentLoop } from '../../src/core/loop.js';
import { Provider, StreamEvent } from '../../src/core/provider.js';
import { AssistantMessage, Message, Tool, ToolContext, ToolDefinition } from '../../src/core/types.js';
import { resolveSessionPath } from '../../src/lib/filesystem.js';

const mockTool: Tool<any> = {
  name: 'mock',
  description: 'Mock tool',
  kind: 'other',
  parameters: z.object({ arg: z.string() }),
  title: 'Run mock',
  execute: async (args: { arg: string }) => ({ content: `Executed: ${args.arg}` }),
};

const mockContext: ToolContext = {
  sessionId: 'session-1',
  cwd: process.cwd(),
  roots: [process.cwd()],
  controlRoot: process.cwd(),
  mode: 'ask',
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
};

class InlineMockProvider implements Provider {
  model = 'mock-model';
  messages: Message[] = [];

  constructor(private readonly responses: AssistantMessage[]) {}

  async complete(
    messages: Message[],
    _tools: ToolDefinition[],
    _systemPrompt?: string,
    _signal?: AbortSignal,
  ): Promise<AssistantMessage> {
    this.messages = [...messages];
    const response = this.responses.shift();
    if (!response) {
      throw new Error('No more mock responses');
    }
    return response;
  }

  async *stream(
    messages: Message[],
    tools: ToolDefinition[],
    systemPrompt?: string,
    signal?: AbortSignal,
  ): AsyncIterable<StreamEvent> {
    yield { type: 'done', message: await this.complete(messages, tools, systemPrompt, signal) };
  }
}

test('agent loop returns the first text response when no tool call is present', async () => {
  const provider = new InlineMockProvider([
    { role: 'assistant', content: [{ type: 'text', text: 'Hello' }] },
  ]);

  const result = await runAgentLoop([], [mockTool], provider, mockContext);
  assert.deepEqual(result, { role: 'assistant', content: [{ type: 'text', text: 'Hello' }] });
});

test('agent loop executes the requested tool and feeds the result back into the conversation', async () => {
  const provider = new InlineMockProvider([
    { role: 'assistant', content: [{ type: 'toolCall', id: 'call-1', name: 'mock', arguments: { arg: 'test' } }] },
    { role: 'assistant', content: [{ type: 'text', text: 'Done' }] },
  ]);

  const result = await runAgentLoop([], [mockTool], provider, mockContext);
  const toolResult = provider.messages.find((message) => message.role === 'toolResult');

  assert.ok(toolResult);
  assert.equal(toolResult.content, 'Executed: test');
  assert.equal(result.content[0]?.type, 'text');
});

test('agent loop rejects invalid tool arguments with a tool result error', async () => {
  const provider = new InlineMockProvider([
    { role: 'assistant', content: [{ type: 'toolCall', id: 'call-1', name: 'mock', arguments: { arg: 123 } }] },
    { role: 'assistant', content: [{ type: 'text', text: 'Done' }] },
  ]);

  await runAgentLoop([], [mockTool], provider, mockContext);
  const toolResult = provider.messages.find((message) => message.role === 'toolResult');

  assert.ok(toolResult);
  assert.equal(toolResult.isError, true);
  assert.match(toolResult.content, /Invalid arguments/);
});

test('agent loop does not resolve tool locations before validating arguments', async () => {
  const guardedTool: Tool<any> = {
    name: 'guarded_read',
    description: 'Read a guarded path',
    kind: 'read',
    parameters: z.object({ path: z.string() }),
    locations: (args, context) => [{ path: resolveSessionPath(args.path, context.cwd, context.roots) }],
    execute: async () => ({ content: 'unreachable' }),
  };
  const provider = new InlineMockProvider([
    { role: 'assistant', content: [{ type: 'toolCall', id: 'call-1', name: 'guarded_read', arguments: { path: 123 } }] },
    { role: 'assistant', content: [{ type: 'text', text: 'Done' }] },
  ]);

  await runAgentLoop([], [guardedTool], provider, mockContext);
  const toolResult = provider.messages.find((message) => message.role === 'toolResult');

  assert.ok(toolResult);
  assert.equal(toolResult.isError, true);
  assert.match(toolResult.content, /Invalid arguments/);
});
