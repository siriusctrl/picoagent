import { z } from 'zod';
import { Provider } from './provider.js';
import { AgentHooks } from './hooks.js';
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
  systemPrompt?: string,
  hooks?: AgentHooks
): Promise<AssistantMessage> {
  const toolDefs = toToolDefinitions(tools);
  let turns = 0;
  
  await hooks?.onLoopStart?.();

  try {
    while (true) {
      turns++;
      
      await hooks?.onLlmStart?.(messages);

      const startLlm = Date.now();
      let response: AssistantMessage | undefined;
      
      if (hooks?.onTextDelta) {
          const stream = provider.stream(messages, toolDefs, systemPrompt);
          for await (const event of stream) {
              if (event.type === 'text_delta' && event.text) {
                  hooks.onTextDelta(event.text);
              } else if (event.type === 'done' && event.message) {
                  response = event.message;
              }
          }
          if (!response) {
            throw new Error("Stream ended without a final message");
          }
      } else {
          response = await provider.complete(messages, toolDefs, systemPrompt);
      }
      
      const llmDuration = Date.now() - startLlm;
      
      await hooks?.onLlmEnd?.(response, llmDuration);

      messages.push(response);

      const toolCalls = response.content.filter(
        (block): block is import('./types.js').ToolCall => block.type === 'toolCall'
      );

      if (toolCalls.length === 0) {
        await hooks?.onLoopEnd?.(turns);
        return response;
      }

      for (const toolCall of toolCalls) {
        await hooks?.onToolStart?.(toolCall);

        const startTool = Date.now();
        let toolResult = await executeTool(toolCall, tools, context);
        const toolDuration = Date.now() - startTool;

        const hookResult = await hooks?.onToolEnd?.(toolCall, toolResult, toolDuration);
        if (hookResult) {
            // Typescript allows void return, so we check if it returned a value
            // but the interface says ToolResultMessage | void.
            // If it returns a modified result, use it.
            // Wait, the interface says: ToolResultMessage | void | Promise<ToolResultMessage | void>
            // So if it returns an object that looks like a ToolResultMessage, we use it.
            if ((hookResult as ToolResultMessage).role === 'toolResult') {
                toolResult = hookResult as ToolResultMessage;
            }
        }

        messages.push(toolResult);
      }
      
      await hooks?.onTurnEnd?.(messages);
    }
  } catch (error) {
    if (error instanceof Error) {
        await hooks?.onError?.(error);
    } else {
        await hooks?.onError?.(new Error(String(error)));
    }
    throw error;
  }
}

// Deprecated: use runAgentLoop with hooks.onTextDelta
export const runAgentLoopStreaming = (
    messages: Message[],
    tools: Tool[],
    provider: Provider,
    context: ToolContext,
    systemPrompt?: string,
    onTextDelta?: (text: string) => void,
    // tracer parameter removed, effectively breaking compatibility if used with tracer
    // but the task says "actually, MERGE the two functions into ONE"
    // so I will implement it as a wrapper for backward compat if needed, 
    // but the instruction implies removing it or merging it.
    // "MERGE the two functions into ONE runAgentLoop"
    // I will keep runAgentLoopStreaming as a wrapper for now to avoid breaking too many tests at once before I fix them,
    // or I can just remove it and fix tests. The instructions say "Refactor src/core/agent-loop.ts".
    // I'll export it for now as a wrapper that constructs hooks.
): Promise<AssistantMessage> => {
    return runAgentLoop(messages, tools, provider, context, systemPrompt, {
        onTextDelta
    });
}
