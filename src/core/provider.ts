import { AssistantMessage, Message, ToolDefinition } from "./types.js";

export interface ProviderConfig {
  apiKey: string;
  model: string;
  maxTokens?: number;
  systemPrompt?: string;
}

export interface Provider {
  complete(messages: Message[], tools: ToolDefinition[], systemPrompt?: string): Promise<AssistantMessage>;
}
