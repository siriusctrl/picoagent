import { test, expect } from 'bun:test';
import { z } from 'zod';
import { runAgentLoop } from '../../src/core/loop.ts';
import { Provider, StreamEvent } from '../../src/core/provider.ts';
import { AssistantMessage, Message, Tool, ToolContext, ToolDefinition } from '../../src/core/types.ts';
import { resolveSessionPath } from '../../src/fs/filesystem.ts';

function requireValue<T>(value: T | undefined, message: string): T {
  if (value === undefined) {
    throw new Error(message);
  }

  return value;
}

const mockTool: Tool<any> = {
  name: 'mock',
  description: 'Mock tool',
  kind: 'other',
  parameters: z.object({ arg: z.string() }),
  title: 'Run mock',
  execute: async (args: { arg: string }) => ({ content: `Executed: ${args.arg}` }),
};

const mockContext: ToolContext = {
  runId: 'run-1',
  sessionId: 'session-1',
  cwd: process.cwd(),
  roots: [process.cwd()],
  controlRoot: process.cwd(),
  signal: new AbortController().signal,
  fileView: {
    glob: async () => [],
    grep: async () => [],
    read: async () => '',
    patch: async () => [],
    cmd: async () => ({
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
  expect(result).toEqual({ role: 'assistant', content: [{ type: 'text', text: 'Hello' }] });
});

test('agent loop executes the requested tool and feeds the result back into the conversation', async () => {
  const provider = new InlineMockProvider([
    { role: 'assistant', content: [{ type: 'toolCall', id: 'call-1', name: 'mock', arguments: { arg: 'test' } }] },
    { role: 'assistant', content: [{ type: 'text', text: 'Done' }] },
  ]);

  const result = await runAgentLoop([], [mockTool], provider, mockContext);
  const toolResult = requireValue(
    provider.messages.find((message) => message.role === 'toolResult'),
    'expected a tool result message',
  );

  expect(toolResult.content).toBe('Executed: test');
  expect(result.content[0]?.type).toBe('text');
});

test('agent loop rejects invalid tool arguments with a tool result error', async () => {
  const provider = new InlineMockProvider([
    { role: 'assistant', content: [{ type: 'toolCall', id: 'call-1', name: 'mock', arguments: { arg: 123 } }] },
    { role: 'assistant', content: [{ type: 'text', text: 'Done' }] },
  ]);

  await runAgentLoop([], [mockTool], provider, mockContext);
  const toolResult = requireValue(
    provider.messages.find((message) => message.role === 'toolResult'),
    'expected a tool result message',
  );

  expect(toolResult.isError).toBeTruthy();
  expect(toolResult.content).toContain('Invalid arguments');
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
  const toolResult = requireValue(
    provider.messages.find((message) => message.role === 'toolResult'),
    'expected a tool result message',
  );

  expect(toolResult.isError).toBeTruthy();
  expect(toolResult.content).toContain('Invalid arguments');
});
