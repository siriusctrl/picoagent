import { z } from 'zod';
import { AgentHooks } from './hooks.ts';
import { Provider } from './provider.ts';
import {
  AssistantMessage,
  ExecutedToolResult,
  Message,
  Tool,
  ToolContext,
  ToolDefinition,
  ToolLocation,
} from './types.ts';

function toToolDefinitions(tools: Tool[]): ToolDefinition[] {
  return tools.map((tool) => ({
    name: tool.name,
    description: tool.description,
    parameters: z.toJSONSchema(tool.parameters) as Record<string, unknown>,
  }));
}

export function truncateOutput(content: string): string {
  if (content.length <= 32000) {
    return content;
  }

  const head = content.slice(0, 24000);
  const tail = content.slice(content.length - 6000);
  return `${head}\n... [${content.length - 30000} chars truncated] ...\n${tail}`;
}

function resolveTitle(tool: Tool | undefined, args: Record<string, unknown>, context: ToolContext): string {
  if (!tool) {
    return 'Unknown tool';
  }

  if (typeof tool.title === 'function') {
    return tool.title(args, context);
  }

  return tool.title ?? tool.name;
}

function fallbackTitle(tool: Tool | undefined): string {
  if (!tool) {
    return 'Unknown tool';
  }

  return typeof tool.title === 'string' ? tool.title : tool.name;
}

function resolveLocations(
  tool: Tool | undefined,
  args: Record<string, unknown>,
  context: ToolContext,
) {
  if (!tool?.locations) {
    return [];
  }

  return tool.locations(args, context);
}

async function executeTool(
  toolCall: import('./types.ts').ToolCall,
  tools: Tool[],
  context: ToolContext,
): Promise<ExecutedToolResult> {
  const tool = tools.find((candidate) => candidate.name === toolCall.name);

  if (!tool) {
    return {
      title: fallbackTitle(tool),
      kind: 'other',
      locations: [],
      message: {
        role: 'toolResult',
        toolCallId: toolCall.id,
        content: 'Tool not found',
        isError: true,
      },
    };
  }

  let title = fallbackTitle(tool);
  let locations: ToolLocation[] = [];

  try {
    const validatedArgs = tool.parameters.parse(toolCall.arguments) as Record<string, unknown>;
    title = resolveTitle(tool, validatedArgs, context);
    locations = resolveLocations(tool, validatedArgs, context);
    const result = await tool.execute(validatedArgs, context);

    return {
      title: result.title ?? title,
      kind: result.kind ?? tool.kind,
      locations: result.locations ?? locations,
      display: result.display,
      rawOutput: result.rawOutput,
      message: {
        role: 'toolResult',
        toolCallId: toolCall.id,
        content: truncateOutput(result.content),
        isError: result.isError ?? false,
      },
    };
  } catch (error: unknown) {
    const content =
      error instanceof z.ZodError
        ? `Invalid arguments: ${error.issues.map((issue) => `${issue.path.join('.')}: ${issue.message}`).join(', ')}`
        : `Error: ${error instanceof Error ? error.message : String(error)}`;

    return {
      title,
      kind: tool.kind,
      locations,
      message: {
        role: 'toolResult',
        toolCallId: toolCall.id,
        content: truncateOutput(content),
        isError: true,
      },
    };
  }
}

export async function runAgentLoop(
  messages: Message[],
  tools: Tool[],
  provider: Provider,
  context: ToolContext,
  systemPrompt?: string,
  hooks?: AgentHooks,
  maxTurns = 24,
): Promise<AssistantMessage> {
  const toolDefinitions = toToolDefinitions(tools);
  let turns = 0;

  await hooks?.onLoopStart?.();

  try {
    while (turns < maxTurns) {
      turns += 1;
      await hooks?.onLlmStart?.(messages);

      const startedAt = Date.now();
      let response: AssistantMessage | undefined;

      if (hooks?.onTextDelta) {
        const stream = provider.stream(messages, toolDefinitions, systemPrompt, context.signal);
        for await (const event of stream) {
          if (event.type === 'text_delta' && event.text) {
            await hooks.onTextDelta(event.text);
          }

          if (event.type === 'done' && event.message) {
            response = event.message;
          }
        }
      } else {
        response = await provider.complete(messages, toolDefinitions, systemPrompt, context.signal);
      }

      if (!response) {
        throw new Error('Provider stream ended without a final message');
      }

      await hooks?.onLlmEnd?.(response, Date.now() - startedAt);
      messages.push(response);

      const toolCalls = response.content.filter(
        (block): block is import('./types.ts').ToolCall => block.type === 'toolCall',
      );

      if (toolCalls.length === 0) {
        await hooks?.onLoopEnd?.(turns);
        return response;
      }

      for (const toolCall of toolCalls) {
        const tool = tools.find((candidate) => candidate.name === toolCall.name);
        await hooks?.onToolStart?.(toolCall, tool);

        const startedToolAt = Date.now();
        let result = await executeTool(toolCall, tools, context);
        const hookResult = await hooks?.onToolEnd?.(toolCall, tool, result, Date.now() - startedToolAt);
        if (hookResult) {
          result = hookResult;
        }

        messages.push(result.message);
      }

      await hooks?.onTurnEnd?.(messages);
    }

    throw new Error(`Agent loop exceeded ${maxTurns} turns`);
  } catch (error) {
    await hooks?.onError?.(error instanceof Error ? error : new Error(String(error)));
    throw error;
  }
}
