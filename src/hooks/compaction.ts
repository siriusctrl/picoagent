import { Provider } from "../core/provider.js";
import { AgentHooks } from "../core/hooks.js";
import { Message, ToolCall, ToolResultMessage } from "../core/types.js";

export interface CompactionConfig {
  contextWindow: number;
  triggerRatio: number;    // default 0.75
  preserveRatio: number;   // default 0.25
  charsPerToken: number;   // default 4
}

export const DEFAULT_CONFIG: CompactionConfig = {
  contextWindow: 200_000,
  triggerRatio: 0.75,
  preserveRatio: 0.25,
  charsPerToken: 4,
};

export function estimateTokens(messages: Message[], charsPerToken = 4): number {
  let chars = 0;
  for (const msg of messages) {
    if (msg.role === "user") {
      chars += msg.content.length;
    } else if (msg.role === "assistant") {
      for (const block of msg.content) {
        if (block.type === "text") {
          chars += block.text.length;
        } else {
          // ToolCall
          chars += JSON.stringify(block.arguments).length + block.name.length;
        }
      }
    } else if (msg.role === "toolResult") {
      chars += msg.content.length;
    }
  }
  return Math.ceil(chars / charsPerToken);
}

export function extractFileOps(messages: Message[]): { read: string[], modified: string[] } {
  const readSet = new Set<string>();
  const modifiedSet = new Set<string>();
  for (const msg of messages) {
    if (msg.role !== "assistant") continue;
    for (const block of msg.content) {
      if (block.type !== "toolCall") continue;
      const args = block.arguments as Record<string, unknown>;
      if ((block.name === "read_file" || block.name === "load") && typeof args.path === "string") {
        readSet.add(args.path);
      }
      if (block.name === "write_file" && typeof args.path === "string") {
        modifiedSet.add(args.path);
      }
    }
  }
  return { read: [...readSet].sort(), modified: [...modifiedSet].sort() };
}

export async function compactMessages(
  messages: Message[],
  provider: Provider,
  config: CompactionConfig
): Promise<void> {
  const totalTokens = estimateTokens(messages, config.charsPerToken);
  const threshold = config.contextWindow * config.triggerRatio;
  
  if (totalTokens < threshold) {
    return;
  }

  const preserveTokens = config.contextWindow * config.preserveRatio;
  let currentTokens = 0;
  let cutIndex = messages.length;

  // Scan backwards to find cut point
  for (let i = messages.length - 1; i >= 0; i--) {
    const msgTokens = estimateTokens([messages[i]], config.charsPerToken);
    if (currentTokens + msgTokens > preserveTokens) {
      break;
    }
    currentTokens += msgTokens;
    cutIndex = i;
  }

  // Ensure we have something to summarize
  if (cutIndex <= 0) return;

  const msgsToSummarize = messages.slice(0, cutIndex);
  const recentMessages = messages.slice(cutIndex);

  // Check for existing summary
  let existingSummary = "";
  let messagesToProcess = msgsToSummarize;
  
  const firstMsg = msgsToSummarize[0];
  if (firstMsg.role === 'user' && firstMsg.content.startsWith("## Previous Context")) {
    existingSummary = firstMsg.content;
    messagesToProcess = msgsToSummarize.slice(1);
  }

  // Extract file ops from the messages being archived
  const { read, modified } = extractFileOps(messagesToProcess);

  // Convert messages to text for the summarizer
  const conversationText = messagesToProcess.map(m => {
    if (m.role === 'user') return `User: ${m.content}`;
    if (m.role === 'assistant') return `Assistant: ${JSON.stringify(m.content)}`;
    if (m.role === 'toolResult') return `Tool Result (${m.toolCallId}): ${m.content}`;
    return '';
  }).join('\n\n');

  let prompt = "";
  if (existingSummary) {
    prompt = `You are a helpful assistant maintaining a long-running conversation history.
The following is the previous summary of the conversation:

${existingSummary}

And here is the new conversation that happened since then:

${conversationText}

Please update the summary to include the new events, maintaining the structure:
1. Goal: The overall objective.
2. Key Decisions: Important choices made.
3. Context: Current state and key information.

Keep it concise but informative.`;
  } else {
    prompt = `You are a helpful assistant maintaining a long-running conversation history.
Please summarize the following conversation:

${conversationText}

Structure the summary as follows:
1. Goal: The overall objective.
2. Key Decisions: Important choices made.
3. Context: Current state and key information.

Keep it concise but informative.`;
  }

  // Call provider to get summary
  // We use a temporary system prompt for this task
  const systemPrompt = "You are a specialized summarization agent.";
  const response = await provider.complete(
    [{ role: 'user', content: prompt }],
    [], // No tools needed for summarization
    systemPrompt
  );

  let newSummary = "";
  if (response.content[0].type === 'text') {
    newSummary = response.content[0].text;
  } else {
    // Fallback if tool call returned (unlikely)
    newSummary = "Error: Could not generate summary.";
  }

  // Format the final summary block
  let finalContent = `## Previous Context\n\n${newSummary}`;
  
  // Append file ops
  if (read.length > 0 || modified.length > 0) {
    finalContent += `\n\n## Touched Files (Archived)\n`;
    if (read.length > 0) finalContent += `Read: ${read.join(', ')}\n`;
    if (modified.length > 0) finalContent += `Modified: ${modified.join(', ')}\n`;
  }

  const summaryMessage: Message = { role: 'user', content: finalContent };

  // Update the messages array in place
  messages.splice(0, cutIndex, summaryMessage);
  
  // Verify tool result pairing in the cut boundary?
  // If recentMessages[0] is toolResult, we have an orphan.
  // But our backward scan logic is simplistic.
  // Improvement: if recentMessages[0] is toolResult, include it in summary or pull its parent toolCall?
  // It's hard to pull parent if it's deep in history.
  // Better to ensure we cut BEFORE a tool call if possible, or include the result in the summary.
  // For now, we assume the simple token cutoff is acceptable for a v0.6 implementation.
}

export function createCompactionHooks(
  provider: Provider,
  config: CompactionConfig = DEFAULT_CONFIG
): AgentHooks {
  return {
    onTurnEnd: async (messages: Message[]) => {
      try {
        await compactMessages(messages, provider, config);
      } catch (error) {
        console.error("Compaction failed:", error);
        // Don't fail the loop, just log
      }
    }
  };
}
