import { Provider } from './provider.js';
import {
  AssistantMessage,
  Message,
  Tool,
  ToolContext,
  ToolResultMessage
} from './types.js';

export async function runAgentLoop(
  messages: Message[],
  tools: Tool[],
  provider: Provider,
  context: ToolContext,
  systemPrompt?: string
): Promise<AssistantMessage> {
  // Note: messages array is mutated to maintain history

  while (true) {
    const response = await provider.complete(messages, tools, systemPrompt);
    messages.push(response);

    const toolCalls = response.content.filter(
      (block): block is import('./types.js').ToolCall => block.type === 'toolCall'
    );

    if (toolCalls.length === 0) {
      return response;
    }

    for (const toolCall of toolCalls) {
      const tool = tools.find(t => t.name === toolCall.name);
      let resultContent = 'Tool not found';
      let isError = true;

      if (tool) {
        try {
          const result = await tool.execute(toolCall.arguments, context);
          resultContent = result.content;
          isError = result.isError || false;
        } catch (error: any) {
          resultContent = `Error: ${error.message}`;
        }
      }

      // Truncate result if too long (head + tail)
      if (resultContent.length > 32000) {
        const head = resultContent.substring(0, 24000);
        const tail = resultContent.substring(resultContent.length - 6000);
        resultContent = `${head}\n... [${resultContent.length - 30000} chars truncated] ...\n${tail}`;
      }

      const toolResult: ToolResultMessage = {
        role: 'toolResult',
        toolCallId: toolCall.id,
        content: resultContent,
        isError
      };
      
      messages.push(toolResult);
    }
  }
}
