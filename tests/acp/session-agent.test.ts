import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { PicoAgent } from '../../src/acp/session-agent.js';
import { Provider, StreamEvent } from '../../src/core/provider.js';
import { AssistantMessage, Message, ToolDefinition } from '../../src/core/types.js';

class InlineMockProvider implements Provider {
  model = 'mock-model';

  constructor(private readonly responses: AssistantMessage[]) {}

  async complete(
    messages: Message[],
    _tools: ToolDefinition[],
    _systemPrompt?: string,
    _signal?: AbortSignal,
  ): Promise<AssistantMessage> {
    const response = this.responses.shift();
    if (!response) {
      throw new Error(`No more mock responses for ${messages.length} messages`);
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

test('ACP sessions report tool failures for out-of-root locations instead of aborting the prompt', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-session-'));
  const previousCwd = process.cwd();
  const previousApiKey = process.env.OPENAI_API_KEY;
  const updates: any[] = [];

  try {
    writeFileSync(join(root, 'config.md'), '---\nprovider: openai\nmodel: gpt-4o\n---\n', 'utf8');
    process.chdir(root);
    process.env.OPENAI_API_KEY = 'test-key';

    const connection = {
      signal: new AbortController().signal,
      sessionUpdate: async (update: unknown) => {
        updates.push(update);
      },
    };
    const agent = new PicoAgent(connection as any);

    (agent as any).bootstrap.provider = new InlineMockProvider([
      {
        role: 'assistant',
        content: [{ type: 'toolCall', id: 'call-1', name: 'read_file', arguments: { path: '../escape.txt' } }],
      },
      { role: 'assistant', content: [{ type: 'text', text: 'Done' }] },
    ]);

    const session = await agent.newSession({ cwd: root } as any);
    const response = await agent.prompt({
      sessionId: session.sessionId,
      prompt: [{ type: 'text', text: 'Read the file.' }],
      messageId: 'message-1',
    } as any);

    const toolCall = updates.find((update) => update.update?.sessionUpdate === 'tool_call');
    const toolCallUpdate = updates.find((update) => update.update?.sessionUpdate === 'tool_call_update');

    assert.equal(response.stopReason, 'end_turn');
    assert.ok(toolCall);
    assert.deepEqual(toolCall.update.locations, []);
    assert.equal(toolCall.update.title, 'read_file');
    assert.ok(toolCallUpdate);
    assert.equal(toolCallUpdate.update.status, 'failed');
    assert.match(toolCallUpdate.update.content[0].content.text, /outside the session roots/);
  } finally {
    process.chdir(previousCwd);
    if (previousApiKey === undefined) {
      delete process.env.OPENAI_API_KEY;
    } else {
      process.env.OPENAI_API_KEY = previousApiKey;
    }
    rmSync(root, { recursive: true, force: true });
  }
});
