import { Provider, StreamEvent } from '../../src/core/provider.js';
import { Message, AssistantMessage, ToolDefinition } from '../../src/core/types.js';

export class MockProvider implements Provider {
    model = 'mock-model';
    messages: Message[] = [];
    responses: AssistantMessage[] = [];
    
    constructor(responses: AssistantMessage[]) {
        this.responses = [...responses]; // Copy
    }

    async complete(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): Promise<AssistantMessage> {
        this.messages = messages; // Store last messages
        const response = this.responses.shift();
        if (!response) {
            // Return a default empty response if ran out, or throw
            // throw new Error("No more responses");
            // For robustness in complex tests, return a simple text
            return { role: 'assistant', content: [{ type: 'text', text: 'Mock response' }] };
        }
        return response;
    }

    async *stream(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): AsyncIterable<StreamEvent> {
        const response = await this.complete(messages, tools, systemPrompt);
        
        // Yield text deltas
        for (const block of response.content) {
            if (block.type === 'text') {
                yield { type: 'text_delta', text: block.text };
            } else {
                yield { type: 'tool_start', toolCall: { id: block.id, name: block.name } };
                // Also yield tool_delta? Simplified for now.
            }
        }
        yield { type: 'done', message: response };
    }
}
