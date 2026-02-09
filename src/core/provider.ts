import { AssistantMessage, Message, ToolDefinition } from "./types.js";

export interface ProviderConfig {
  apiKey: string;
  model: string;
  maxTokens?: number;
  systemPrompt?: string;
}

export interface StreamEvent {
  type: "text_delta" | "tool_start" | "tool_delta" | "done" | "error";
  // text_delta
  text?: string;
  // tool_start
  toolCall?: { id: string; name: string };
  // done
  message?: AssistantMessage;
  // error
  error?: string;
}

export interface Provider {
  model: string;
  complete(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): Promise<AssistantMessage>;
  stream(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): AsyncIterable<StreamEvent>;
}
