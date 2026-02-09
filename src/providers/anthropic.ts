import Anthropic from '@anthropic-ai/sdk';
import {
  AssistantMessage,
  Message,
  TextContent,
  ToolCall,
  ToolDefinition
} from '../core/types.js';
import { Provider, ProviderConfig } from '../core/provider.js';

export class AnthropicProvider implements Provider {
  private client: Anthropic;
  private config: ProviderConfig;

  constructor(config: ProviderConfig) {
    this.config = config;
    this.client = new Anthropic({ apiKey: config.apiKey });
  }

  async complete(
    messages: Message[],
    tools: ToolDefinition[],
    systemPrompt?: string
  ): Promise<AssistantMessage> {
    const system = systemPrompt || this.config.systemPrompt;

    const anthropicMessages: Anthropic.MessageParam[] = [];

    for (const m of messages) {
      if (m.role === 'user') {
        anthropicMessages.push({ role: 'user', content: m.content });
      } else if (m.role === 'assistant') {
        anthropicMessages.push({
          role: 'assistant',
          content: m.content.map(c =>
            c.type === 'text'
              ? { type: 'text' as const, text: c.text }
              : { type: 'tool_use' as const, id: c.id, name: c.name, input: c.arguments }
          )
        });
      } else if (m.role === 'toolResult') {
        const block = {
          type: 'tool_result' as const,
          tool_use_id: m.toolCallId,
          content: m.content,
          is_error: m.isError
        };

        // Anthropic expects consecutive tool results grouped in a single user message
        const last = anthropicMessages[anthropicMessages.length - 1];
        if (last?.role === 'user' && Array.isArray(last.content)
            && (last.content[0] as any)?.type === 'tool_result') {
          (last.content as any[]).push(block);
        } else {
          anthropicMessages.push({ role: 'user', content: [block] });
        }
      }
    }

    const anthropicTools: Anthropic.Tool[] = tools.map(t => ({
      name: t.name,
      description: t.description,
      input_schema: t.parameters as Anthropic.Tool.InputSchema
    }));

    const response = await this.client.messages.create({
      model: this.config.model,
      max_tokens: this.config.maxTokens || 4096,
      system,
      messages: anthropicMessages,
      tools: anthropicTools,
    });

    const content: (TextContent | ToolCall)[] = response.content.map(block => {
      if (block.type === 'text') {
        return { type: 'text', text: block.text };
      }
      if (block.type === 'tool_use') {
        return {
          type: 'toolCall',
          id: block.id,
          name: block.name,
          arguments: block.input as Record<string, unknown>
        };
      }
      throw new Error(`Unknown block type: ${block.type}`);
    });

    return {
      role: 'assistant',
      content
    };
  }
}
