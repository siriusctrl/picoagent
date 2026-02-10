import OpenAI from 'openai';
import { z } from 'zod';
import {
  AssistantMessage,
  Message,
  TextContent,
  ToolCall,
  ToolDefinition
} from '../core/types.js';
import { Provider, ProviderConfig, StreamEvent } from '../core/provider.js';

export interface OpenAIProviderConfig extends ProviderConfig {
  baseURL?: string;  // override for compatible APIs (DeepSeek, Groq, Together, Ollama, etc.)
}

// === Response validation schemas (trust boundary) ===

const ToolCallSchema = z.object({
  id: z.string(),
  type: z.literal('function'),
  function: z.object({
    name: z.string(),
    arguments: z.string()
  })
});

const ChoiceMessageSchema = z.object({
  role: z.literal('assistant'),
  content: z.string().nullable().optional(),
  tool_calls: z.array(ToolCallSchema).optional()
});

export class OpenAIProvider implements Provider {
  private client: OpenAI;
  private config: OpenAIProviderConfig;

  constructor(config: OpenAIProviderConfig) {
    this.config = config;
    this.client = new OpenAI({
      apiKey: config.apiKey,
      baseURL: config.baseURL,
    });
  }

  get model(): string {
    return this.config.model;
  }

  private convertMessages(messages: Message[]): OpenAI.ChatCompletionMessageParam[] {
    const result: OpenAI.ChatCompletionMessageParam[] = [];

    for (const m of messages) {
      if (m.role === 'user') {
        result.push({ role: 'user', content: m.content });
      } else if (m.role === 'assistant') {
        const textParts = m.content.filter(c => c.type === 'text');
        const toolCalls = m.content.filter(c => c.type === 'toolCall');

        const msg: OpenAI.ChatCompletionAssistantMessageParam = {
          role: 'assistant',
          content: textParts.length > 0
            ? textParts.map(c => c.text).join('')
            : null,
        };

        if (toolCalls.length > 0) {
          msg.tool_calls = toolCalls.map(c => ({
            id: c.id,
            type: 'function' as const,
            function: {
              name: c.name,
              arguments: JSON.stringify(c.arguments)
            }
          }));
        }

        result.push(msg);
      } else if (m.role === 'toolResult') {
        result.push({
          role: 'tool',
          tool_call_id: m.toolCallId,
          content: m.content
        });
      }
    }

    return result;
  }

  private convertTools(tools: ToolDefinition[]): OpenAI.ChatCompletionTool[] {
    return tools.map(t => ({
      type: 'function' as const,
      function: {
        name: t.name,
        description: t.description,
        parameters: t.parameters
      }
    }));
  }

  private parseResponse(raw: unknown): AssistantMessage {
    const validated = ChoiceMessageSchema.parse(raw);

    const content: (TextContent | ToolCall)[] = [];

    if (validated.content) {
      content.push({ type: 'text', text: validated.content });
    }

    if (validated.tool_calls) {
      for (const tc of validated.tool_calls) {
        let args: Record<string, unknown>;
        try {
          args = JSON.parse(tc.function.arguments);
        } catch {
          args = {};
        }
        content.push({
          type: 'toolCall',
          id: tc.id,
          name: tc.function.name,
          arguments: args
        });
      }
    }

    return { role: 'assistant', content };
  }

  async complete(
    messages: Message[],
    tools: ToolDefinition[],
    systemPrompt?: string
  ): Promise<AssistantMessage> {
    const system = systemPrompt || this.config.systemPrompt;

    const openaiMessages = this.convertMessages(messages);
    if (system) {
      openaiMessages.unshift({ role: 'system', content: system });
    }

    const response = await this.client.chat.completions.create({
      model: this.config.model,
      max_tokens: this.config.maxTokens || 4096,
      messages: openaiMessages,
      tools: tools.length > 0 ? this.convertTools(tools) : undefined,
    });

    return this.parseResponse(response.choices[0].message);
  }

  async *stream(
    messages: Message[],
    tools: ToolDefinition[],
    systemPrompt?: string
  ): AsyncIterable<StreamEvent> {
    const system = systemPrompt || this.config.systemPrompt;

    const openaiMessages = this.convertMessages(messages);
    if (system) {
      openaiMessages.unshift({ role: 'system', content: system });
    }

    const stream = await this.client.chat.completions.create({
      model: this.config.model,
      max_tokens: this.config.maxTokens || 4096,
      messages: openaiMessages,
      tools: tools.length > 0 ? this.convertTools(tools) : undefined,
      stream: true,
    });

    // Accumulate the full response for the final message
    let currentText = '';
    const toolCalls = new Map<number, { id: string; name: string; args: string }>();

    for await (const chunk of stream) {
      const delta = chunk.choices[0]?.delta;
      if (!delta) continue;

      if (delta.content) {
        currentText += delta.content;
        yield { type: 'text_delta', text: delta.content };
      }

      if (delta.tool_calls) {
        for (const tc of delta.tool_calls) {
          const idx = tc.index;
          if (!toolCalls.has(idx)) {
            toolCalls.set(idx, { id: tc.id || '', name: tc.function?.name || '', args: '' });
            if (tc.id && tc.function?.name) {
              yield { type: 'tool_start', toolCall: { id: tc.id, name: tc.function.name } };
            }
          }
          const existing = toolCalls.get(idx)!;
          if (tc.id) existing.id = tc.id;
          if (tc.function?.name) existing.name = tc.function.name;
          if (tc.function?.arguments) existing.args += tc.function.arguments;
        }
      }
    }

    // Build final message
    const content: (TextContent | ToolCall)[] = [];
    if (currentText) {
      content.push({ type: 'text', text: currentText });
    }
    for (const tc of toolCalls.values()) {
      let args: Record<string, unknown>;
      try {
        args = JSON.parse(tc.args);
      } catch {
        args = {};
      }
      content.push({
        type: 'toolCall',
        id: tc.id,
        name: tc.name,
        arguments: args
      });
    }

    yield { type: 'done', message: { role: 'assistant', content } };
  }
}
