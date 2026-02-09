import { z } from 'zod';
import { Provider } from './provider.js';
import { Tracer } from './trace.js';
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
  tracer?: Tracer
): Promise<AssistantMessage> {
  const toolDefs = toToolDefinitions(tools);
  let turns = 0;
  
  let agentSpanId: string | undefined;
  if (tracer) {
    agentSpanId = tracer.span();
    tracer.emit({ 
        event: 'agent_start', 
        span_id: agentSpanId,
        data: { 
        model: provider.model, 
        tool_count: tools.length 
        } 
    });
  }

  try {
    while (true) {
      turns++;
      let llmSpanId: string | undefined;
      
      if (tracer && agentSpanId) {
        llmSpanId = tracer.span(agentSpanId);
        tracer.emit({ 
            event: 'llm_start', 
            span_id: llmSpanId, 
            parent_span: agentSpanId,
            data: { message_count: messages.length } 
        });
      }

      const startLlm = Date.now();
      const response = await provider.complete(messages, toolDefs, systemPrompt);
      const llmDuration = Date.now() - startLlm;
      
      if (tracer && llmSpanId) {
        tracer.emit({ 
            event: 'llm_end', 
            span_id: llmSpanId,
            parent_span: agentSpanId,
            duration_ms: llmDuration 
        });
      }

      messages.push(response);

      const toolCalls = response.content.filter(
        (block): block is import('./types.js').ToolCall => block.type === 'toolCall'
      );

      if (toolCalls.length === 0) {
        if (tracer && agentSpanId) {
            tracer.emit({ 
                event: 'agent_end', 
                span_id: agentSpanId,
                data: { total_turns: turns } 
            });
        }
        return response;
      }

      for (const toolCall of toolCalls) {
        let toolSpanId: string | undefined;
        if (tracer && llmSpanId) {
            toolSpanId = tracer.span(llmSpanId);
            tracer.emit({ 
                event: 'tool_start', 
                span_id: toolSpanId, 
                parent_span: llmSpanId,
                data: { tool: toolCall.name, args: toolCall.arguments } 
            });
        }

        const startTool = Date.now();
        const toolResult = await executeTool(toolCall, tools, context);
        const toolDuration = Date.now() - startTool;

        if (tracer && toolSpanId && llmSpanId) {
            tracer.emit({ 
                event: 'tool_end', 
                span_id: toolSpanId, 
                parent_span: llmSpanId,
                duration_ms: toolDuration,
                data: { 
                result_length: toolResult.content.length, 
                isError: toolResult.isError 
                } 
            });
        }

        messages.push(toolResult);
      }
    }
  } catch (error) {
    if (tracer && agentSpanId) {
        tracer.emit({ 
            event: 'error', 
            span_id: agentSpanId,
            data: { message: error instanceof Error ? error.message : String(error) } 
        });
    }
    throw error;
  }
}

export async function runAgentLoopStreaming(
  messages: Message[],
  tools: Tool[],
  provider: Provider,
  context: ToolContext,
  systemPrompt?: string,
  onTextDelta?: (text: string) => void,
  tracer?: Tracer
): Promise<AssistantMessage> {
  const toolDefs = toToolDefinitions(tools);
  let turns = 0;

  let agentSpanId: string | undefined;
  if (tracer) {
    agentSpanId = tracer.span();
    tracer.emit({ 
        event: 'agent_start', 
        span_id: agentSpanId,
        data: { 
        model: provider.model, 
        tool_count: tools.length 
        } 
    });
  }

  try {
    while (true) {
        turns++;
        let llmSpanId: string | undefined;

        if (tracer && agentSpanId) {
            llmSpanId = tracer.span(agentSpanId);
            tracer.emit({ 
                event: 'llm_start', 
                span_id: llmSpanId, 
                parent_span: agentSpanId,
                data: { message_count: messages.length } 
            });
        }
        
        const startLlm = Date.now();
        let response: AssistantMessage | undefined;
        const stream = provider.stream(messages, toolDefs, systemPrompt);
        
        for await (const event of stream) {
            if (event.type === 'text_delta' && event.text) {
                onTextDelta?.(event.text);
            } else if (event.type === 'done' && event.message) {
                response = event.message;
            }
        }
        
        const llmDuration = Date.now() - startLlm;

        if (!response) {
            throw new Error("Stream ended without a final message");
        }

        if (tracer && llmSpanId) {
            tracer.emit({ 
                event: 'llm_end', 
                span_id: llmSpanId, 
                parent_span: agentSpanId,
                duration_ms: llmDuration 
            });
        }

        messages.push(response);

        const toolCalls = response.content.filter(
        (block): block is import('./types.js').ToolCall => block.type === 'toolCall'
        );

        if (toolCalls.length === 0) {
            if (tracer && agentSpanId) {
                tracer.emit({ 
                    event: 'agent_end', 
                    span_id: agentSpanId,
                    data: { total_turns: turns } 
                });
            }
            return response;
        }

        for (const toolCall of toolCalls) {
            let toolSpanId: string | undefined;
            if (tracer && llmSpanId) {
                toolSpanId = tracer.span(llmSpanId);
                tracer.emit({ 
                    event: 'tool_start', 
                    span_id: toolSpanId, 
                    parent_span: llmSpanId,
                    data: { tool: toolCall.name, args: toolCall.arguments } 
                });
            }

            const startTool = Date.now();
            const toolResult = await executeTool(toolCall, tools, context);
            const toolDuration = Date.now() - startTool;

            if (tracer && toolSpanId && llmSpanId) {
                tracer.emit({ 
                    event: 'tool_end', 
                    span_id: toolSpanId, 
                    parent_span: llmSpanId,
                    duration_ms: toolDuration,
                    data: { 
                    result_length: toolResult.content.length, 
                    isError: toolResult.isError 
                    } 
                });
            }

            messages.push(toolResult);
        }
    }
  } catch (error) {
    if (tracer && agentSpanId) {
        tracer.emit({ 
            event: 'error', 
            span_id: agentSpanId,
            data: { message: error instanceof Error ? error.message : String(error) } 
        });
    }
    throw error;
  }
}
