import { AssistantMessage, Message, ToolDefinition } from '../core/types.ts';
import { Provider, ProviderConfig, StreamEvent } from '../core/provider.ts';

function lastUserContent(messages: Message[]): string {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role === 'user') {
      return message.content;
    }
  }

  return '';
}

function chunkText(text: string, chunkSize = 12): string[] {
  const chunks: string[] = [];
  for (let index = 0; index < text.length; index += chunkSize) {
    chunks.push(text.slice(index, index + chunkSize));
  }

  return chunks;
}

export class EchoProvider implements Provider {
  constructor(private readonly config: ProviderConfig) {}

  get model(): string {
    return this.config.model;
  }

  private buildResponse(messages: Message[]): AssistantMessage {
    const echoed = `received: ${lastUserContent(messages)}`;
    return {
      role: 'assistant',
      content: [{ type: 'text', text: echoed }],
    };
  }

  async complete(
    messages: Message[],
    _tools: ToolDefinition[],
    _systemPrompt?: string,
    _signal?: AbortSignal,
  ): Promise<AssistantMessage> {
    return this.buildResponse(messages);
  }

  async *stream(
    messages: Message[],
    _tools: ToolDefinition[],
    _systemPrompt?: string,
    signal?: AbortSignal,
  ): AsyncIterable<StreamEvent> {
    const response = this.buildResponse(messages);
    const text = response.content[0]?.type === 'text' ? response.content[0].text : '';

    for (const chunk of chunkText(text)) {
      if (signal?.aborted) {
        throw new Error('Echo provider stream aborted');
      }

      yield { type: 'text_delta', text: chunk };
    }

    yield { type: 'done', message: response };
  }
}
