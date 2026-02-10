import { GoogleGenAI, Type } from '@google/genai';
import { z } from 'zod';
import {
  AssistantMessage,
  Message,
  TextContent,
  ToolCall,
  ToolDefinition
} from '../core/types.js';
import { Provider, ProviderConfig, StreamEvent } from '../core/provider.js';

// === Response validation schemas (trust boundary) ===

const FunctionCallSchema = z.object({
  name: z.string(),
  args: z.record(z.string(), z.unknown()).optional()
});

const PartSchema = z.union([
  z.object({ text: z.string() }),
  z.object({ functionCall: FunctionCallSchema })
]);

export class GeminiProvider implements Provider {
  private client: GoogleGenAI;
  private config: ProviderConfig;

  constructor(config: ProviderConfig) {
    this.config = config;
    this.client = new GoogleGenAI({ apiKey: config.apiKey });
  }

  get model(): string {
    return this.config.model;
  }

  private convertHistory(messages: Message[]) {
    const contents: Array<{ role: string; parts: Array<Record<string, unknown>> }> = [];

    for (const m of messages) {
      if (m.role === 'user') {
        contents.push({ role: 'user', parts: [{ text: m.content }] });
      } else if (m.role === 'assistant') {
        const parts: Array<Record<string, unknown>> = [];
        for (const c of m.content) {
          if (c.type === 'text') {
            parts.push({ text: c.text });
          } else {
            parts.push({
              functionCall: { name: c.name, args: c.arguments }
            });
          }
        }
        contents.push({ role: 'model', parts });
      } else if (m.role === 'toolResult') {
        contents.push({
          role: 'user',
          parts: [{
            functionResponse: {
              name: m.toolCallId,  // Gemini uses the function name, but we use toolCallId
              response: { result: m.content }
            }
          }]
        });
      }
    }

    return contents;
  }

  private convertJsonSchemaToGemini(schema: Record<string, unknown>): Record<string, unknown> {
    // Gemini uses a subset of JSON Schema with some differences
    // Convert standard JSON Schema to Gemini's format
    const result: Record<string, unknown> = {};

    if (schema.type) {
      const typeMap: Record<string, string> = {
        'string': Type.STRING,
        'number': Type.NUMBER,
        'integer': Type.INTEGER,
        'boolean': Type.BOOLEAN,
        'array': Type.ARRAY,
        'object': Type.OBJECT,
      };
      result.type = typeMap[schema.type as string] || schema.type;
    }

    if (schema.description) result.description = schema.description;
    if (schema.enum) result.enum = schema.enum;

    if (schema.properties) {
      const props: Record<string, unknown> = {};
      for (const [key, value] of Object.entries(schema.properties as Record<string, unknown>)) {
        props[key] = this.convertJsonSchemaToGemini(value as Record<string, unknown>);
      }
      result.properties = props;
    }

    if (schema.required) result.required = schema.required;

    if (schema.items) {
      result.items = this.convertJsonSchemaToGemini(schema.items as Record<string, unknown>);
    }

    return result;
  }

  private convertTools(tools: ToolDefinition[]) {
    if (tools.length === 0) return undefined;

    return [{
      functionDeclarations: tools.map(t => ({
        name: t.name,
        description: t.description,
        parameters: this.convertJsonSchemaToGemini(t.parameters)
      }))
    }];
  }

  private parseResponse(parts: unknown[]): AssistantMessage {
    const content: (TextContent | ToolCall)[] = [];
    let callIndex = 0;

    for (const rawPart of parts) {
      const parsed = PartSchema.parse(rawPart);

      if ('text' in parsed) {
        content.push({ type: 'text', text: parsed.text });
      } else if ('functionCall' in parsed) {
        content.push({
          type: 'toolCall',
          id: `call_${callIndex++}`,
          name: parsed.functionCall.name,
          arguments: (parsed.functionCall.args || {}) as Record<string, unknown>
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
    const contents = this.convertHistory(messages);

    const response = await this.client.models.generateContent({
      model: this.config.model,
      contents,
      config: {
        systemInstruction: system,
        tools: this.convertTools(tools),
        maxOutputTokens: this.config.maxTokens || 4096,
      }
    });

    const parts = response.candidates?.[0]?.content?.parts || [];
    return this.parseResponse(parts);
  }

  async *stream(
    messages: Message[],
    tools: ToolDefinition[],
    systemPrompt?: string
  ): AsyncIterable<StreamEvent> {
    const system = systemPrompt || this.config.systemPrompt;
    const contents = this.convertHistory(messages);

    const response = await this.client.models.generateContentStream({
      model: this.config.model,
      contents,
      config: {
        systemInstruction: system,
        tools: this.convertTools(tools),
        maxOutputTokens: this.config.maxTokens || 4096,
      }
    });

    const allParts: unknown[] = [];

    for await (const chunk of response) {
      const parts = chunk.candidates?.[0]?.content?.parts || [];

      for (const part of parts) {
        allParts.push(part);

        if (typeof (part as any).text === 'string') {
          yield { type: 'text_delta', text: (part as any).text };
        } else if ((part as any).functionCall) {
          const fc = (part as any).functionCall;
          yield { type: 'tool_start', toolCall: { id: `call_${allParts.length}`, name: fc.name } };
        }
      }
    }

    yield { type: 'done', message: this.parseResponse(allParts) };
  }
}
