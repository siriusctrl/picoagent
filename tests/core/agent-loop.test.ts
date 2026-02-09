import { test } from 'node:test';
import assert from 'node:assert';
import { runAgentLoop, runAgentLoopStreaming } from '../../src/core/agent-loop.js';
import { Provider, StreamEvent } from '../../src/core/provider.js';
import { Message, Tool, ToolContext, AssistantMessage, ToolDefinition } from '../../src/core/types.js';
import { z } from 'zod';

const mockTool: Tool<any> = {
  name: 'mock',
  description: 'Mock tool',
  parameters: z.object({ arg: z.string() }),
  execute: async (args: { arg: string }) => ({ content: `Executed: ${args.arg}` })
};

const errorTool: Tool<any> = {
  name: 'error',
  description: 'Error tool',
  parameters: z.object({}),
  execute: async () => { throw new Error('Tool failure'); }
};

const mockTools = [mockTool, errorTool];
const mockContext: ToolContext = { cwd: process.cwd(), tasksRoot: process.cwd() };

class MockProvider implements Provider {
    model = 'mock-model';
    messages: Message[] = [];
    responses: AssistantMessage[] = [];
    
    constructor(responses: AssistantMessage[]) {
        this.responses = responses;
    }

    async complete(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): Promise<AssistantMessage> {
        this.messages = messages;
        const response = this.responses.shift();
        if (!response) throw new Error("No more responses");
        return response;
    }

    async *stream(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): AsyncIterable<StreamEvent> {
        const response = await this.complete(messages, tools, systemPrompt);
        for (const block of response.content) {
            if (block.type === 'text') {
                yield { type: 'text_delta', text: block.text };
            } else {
                yield { type: 'tool_start', toolCall: { id: block.id, name: block.name } };
            }
        }
        yield { type: 'done', message: response };
    }
}

test('agent loop simple text response', async () => {
    const provider = new MockProvider([{ 
        role: 'assistant', 
        content: [{ type: 'text', text: 'Hello' }] 
    }]);
    
    const result = await runAgentLoop([], mockTools, provider, mockContext);
    assert.strictEqual(result.content[0].type, 'text');
    assert.strictEqual((result.content[0] as any).text, 'Hello');
});

test('agent loop tool execution', async () => {
    const provider = new MockProvider([
        { 
            role: 'assistant', 
            content: [{ type: 'toolCall', id: '1', name: 'mock', arguments: { arg: 'test' } }] 
        },
        { 
            role: 'assistant', 
            content: [{ type: 'text', text: 'Done' }] 
        }
    ]);
    
    const result = await runAgentLoop([], mockTools, provider, mockContext);
    
    const toolResultMsg = provider.messages.find(m => m.role === 'toolResult');
    assert.ok(toolResultMsg);
    assert.strictEqual(toolResultMsg.content, 'Executed: test');
    assert.strictEqual((result.content[0] as any).text, 'Done');
});

test('agent loop invalid tool args', async () => {
    const provider = new MockProvider([
        { 
            role: 'assistant', 
            content: [{ type: 'toolCall', id: '1', name: 'mock', arguments: { arg: 123 } }] // Invalid arg type
        },
        { 
            role: 'assistant', 
            content: [{ type: 'text', text: 'Done' }] 
        }
    ]);
    
    await runAgentLoop([], mockTools, provider, mockContext);
    
    const toolResultMsg = provider.messages.find(m => m.role === 'toolResult');
    assert.ok(toolResultMsg);
    assert.ok(toolResultMsg.content.includes('Invalid arguments'));
    assert.strictEqual(toolResultMsg.isError, true);
});

test('agent loop unknown tool', async () => {
    const provider = new MockProvider([
        { 
            role: 'assistant', 
            content: [{ type: 'toolCall', id: '1', name: 'unknown', arguments: {} }] 
        },
        { 
            role: 'assistant', 
            content: [{ type: 'text', text: 'Done' }] 
        }
    ]);
    
    await runAgentLoop([], mockTools, provider, mockContext);
    
    const toolResultMsg = provider.messages.find(m => m.role === 'toolResult');
    assert.ok(toolResultMsg);
    assert.strictEqual(toolResultMsg.content, 'Tool not found');
    assert.strictEqual(toolResultMsg.isError, true);
});

test('agent loop tool truncation', async () => {
    const largeTool: Tool<any> = {
        name: 'large',
        description: 'Large output',
        parameters: z.object({}),
        execute: async () => ({ content: 'a'.repeat(33000) })
    };
    
    const provider = new MockProvider([
        { 
            role: 'assistant', 
            content: [{ type: 'toolCall', id: '1', name: 'large', arguments: {} }] 
        },
        { 
            role: 'assistant', 
            content: [{ type: 'text', text: 'Done' }] 
        }
    ]);
    
    await runAgentLoop([], [largeTool], provider, mockContext);
    
    const toolResultMsg = provider.messages.find(m => m.role === 'toolResult');
    assert.ok(toolResultMsg);
    assert.ok(toolResultMsg.content.includes('chars truncated'));
    assert.strictEqual(toolResultMsg.content.length < 33000, true);
});
