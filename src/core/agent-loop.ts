import { z } from 'zod';
import { Provider } from './provider.js';
import {
  AssistantMessage,
  Message,
  Tool,
  ToolContext,
  ToolDefinition,
  ToolResultMessage
} from './types.js';

/** Convert Tool[] (Zod schemas) to ToolDefinition[] (JSON Schema) for the provider */
function toToolDefinitions(tools: Tool[]): ToolDefinition[] {
  return tools.map(t => ({
    name: t.name,
    description: t.description,
    parameters: z.toJSONSchema(t.parameters) as Record<string, unknown>
  }));
}

export function truncateOutput(content: string): string {
  if (content.length > 32000) {
    const head = content.substring(0, 24000);
    const tail = content.substring(content.length - 6000);
    return `${head}\n... [${content.length - 30000} chars truncated] ...\n${tail}`;
  }
  return content;
}

async function executeTool(
    toolCall: import('./types.js').ToolCall,
    tools: Tool[],
    context: ToolContext
): Promise<ToolResultMessage> {
      const tool = tools.find(t => t.name === toolCall.name);
      let resultContent = 'Tool not found';
      let isError = true;

      if (tool) {
        try {
          // Validate LLM-generated args through Zod schema
          const validatedArgs = tool.parameters.parse(toolCall.arguments);
          const result = await tool.execute(validatedArgs, context);
          resultContent = result.content;
          isError = result.isError || false;
        } catch (error: unknown) {
          if (error instanceof z.ZodError) {
            resultContent = `Invalid arguments: ${error.issues.map(i => `${i.path.join('.')}: ${i.message}`).join(', ')}`;
          } else {
            resultContent = `Error: ${error instanceof Error ? error.message : String(error)}`;
          }
        }
      }
      
      return {
        role: 'toolResult',
        toolCallId: toolCall.id,
        content: truncateOutput(resultContent),
        isError
      };
}

export async function runAgentLoop(
  messages: Message[],
  tools: Tool[],
  provider: Provider,
  context: ToolContext,
  systemPrompt?: string
): Promise<AssistantMessage> {
  const toolDefs = toToolDefinitions(tools);

  while (true) {
    const response = await provider.complete(messages, toolDefs, systemPrompt);
    messages.push(response);

    const toolCalls = response.content.filter(
      (block): block is import('./types.js').ToolCall => block.type === 'toolCall'
    );

    if (toolCalls.length === 0) {
      return response;
    }

    for (const toolCall of toolCalls) {
      const toolResult = await executeTool(toolCall, tools, context);
      messages.push(toolResult);
    }
  }
}

export async function runAgentLoopStreaming(
  messages: Message[],
  tools: Tool[],
  provider: Provider,
  context: ToolContext,
  systemPrompt?: string,
  onTextDelta?: (text: string) => void
): Promise<AssistantMessage> {
  const toolDefs = toToolDefinitions(tools);

  while (true) {
    let response: AssistantMessage | undefined;
    const stream = provider.stream(messages, toolDefs, systemPrompt);
    
    for await (const event of stream) {
        if (event.type === 'text_delta' && event.text) {
            onTextDelta?.(event.text);
        } else if (event.type === 'done' && event.message) {
            response = event.message;
        }
    }
    
    if (!response) {
        throw new Error("Stream ended without a final message");
    }

    messages.push(response);

    const toolCalls = response.content.filter(
      (block): block is import('./types.js').ToolCall => block.type === 'toolCall'
    );

    if (toolCalls.length === 0) {
      return response;
    }

    for (const toolCall of toolCalls) {
      const toolResult = await executeTool(toolCall, tools, context);
      messages.push(toolResult);
    }
  }
}
