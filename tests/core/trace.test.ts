import { test } from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, readFileSync, rmSync, existsSync } from 'fs';
import { join } from 'path';
import { tmpdir } from 'os';
import { Tracer, TraceEvent } from '../../src/core/trace.js';
import { createTraceHooks } from '../../src/core/trace-hooks.js';
import { runAgentLoop } from '../../src/core/agent-loop.js';
import { Provider, StreamEvent } from '../../src/core/provider.js';
import { Message, Tool, ToolContext, AssistantMessage, ToolDefinition } from '../../src/core/types.js';
import { z } from 'zod';

const mockTool: Tool<any> = {
  name: 'mock',
  description: 'Mock tool',
  parameters: z.object({ arg: z.string() }),
  execute: async (args: { arg: string }) => ({ content: `Executed: ${args.arg}` })
};

const context: ToolContext = { cwd: process.cwd(), tasksRoot: process.cwd() };

class MockProvider implements Provider {
    model = 'mock-model';
    responses: AssistantMessage[] = [];
    
    constructor(responses: AssistantMessage[]) {
        this.responses = responses;
    }

    async complete(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): Promise<AssistantMessage> {
        const response = this.responses.shift();
        if (!response) throw new Error("No more responses");
        return response;
    }

    async *stream(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): AsyncIterable<StreamEvent> {
         // Not needed for this test
         yield { type: 'done' };
    }
}

test('Tracer creates file on first emit', () => {
    const traceDir = mkdtempSync(join(tmpdir(), 'picoagent-trace-'));
    const tracer = new Tracer(traceDir);
    
    tracer.emit({ 
        event: 'agent_start', 
        span_id: tracer.span(),
        data: { test: true } 
    });

    const filePath = join(traceDir, `${tracer.traceId}.jsonl`);
    assert.ok(existsSync(filePath));
    
    rmSync(traceDir, { recursive: true, force: true });
});

test('Tracer writes valid JSON lines', () => {
    const traceDir = mkdtempSync(join(tmpdir(), 'picoagent-trace-'));
    const tracer = new Tracer(traceDir);
    const spanId = tracer.span();
    
    tracer.emit({ event: 'start', span_id: spanId, data: { step: 1 } });
    tracer.emit({ event: 'end', span_id: spanId, data: { step: 2 } });

    const filePath = join(traceDir, `${tracer.traceId}.jsonl`);
    const content = readFileSync(filePath, 'utf-8');
    const lines = content.trim().split('\n');
    
    assert.strictEqual(lines.length, 2);
    
    const event1 = JSON.parse(lines[0]) as TraceEvent;
    assert.strictEqual(event1.event, 'start');
    assert.strictEqual(event1.trace_id, tracer.traceId);
    assert.strictEqual(event1.span_id, spanId);
    assert.ok(event1.timestamp);
    
    const event2 = JSON.parse(lines[1]) as TraceEvent;
    assert.strictEqual(event2.event, 'end');

    rmSync(traceDir, { recursive: true, force: true });
});

test('Tracer integrates with agent loop', async () => {
    const traceDir = mkdtempSync(join(tmpdir(), 'picoagent-trace-'));
    const tracer = new Tracer(traceDir);
    
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
    
    const hooks = createTraceHooks(tracer, 'mock-model');
    await runAgentLoop([], [mockTool], provider, context, undefined, hooks);
    
    const filePath = join(traceDir, `${tracer.traceId}.jsonl`);
    const content = readFileSync(filePath, 'utf-8');
    const events = content.trim().split('\n').map(l => JSON.parse(l)) as TraceEvent[];
    
    // Check sequence of events
    const eventTypes = events.map(e => e.event);
    assert.ok(eventTypes.includes('agent_start'));
    assert.ok(eventTypes.includes('llm_start'));
    assert.ok(eventTypes.includes('llm_end'));
    assert.ok(eventTypes.includes('tool_start'));
    assert.ok(eventTypes.includes('tool_end'));
    assert.ok(eventTypes.includes('agent_end'));
    
    // Verify parent-child relationships
    const toolStart = events.find(e => e.event === 'tool_start');
    const llmEnd = events.find(e => e.event === 'llm_end');
    
    assert.ok(toolStart?.parent_span);
    // Note: toolStart parent should be the LLM call that generated it. 
    // In our implementation, we use the LLM span ID as parent for tool execution.
    // The LLM end event also has that span ID.
    assert.strictEqual(toolStart?.parent_span, llmEnd?.span_id);
    
    rmSync(traceDir, { recursive: true, force: true });
});
