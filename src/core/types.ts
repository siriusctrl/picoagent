export interface TextContent {
  type: "text";
  text: string;
}

export interface ToolCall {
  type: "toolCall";
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export interface UserMessage {
  role: "user";
  content: string;
}

export interface AssistantMessage {
  role: "assistant";
  content: (TextContent | ToolCall)[];
}

export interface ToolResultMessage {
  role: "toolResult";
  toolCallId: string;
  content: string;
  isError: boolean;
}

export type Message = UserMessage | AssistantMessage | ToolResultMessage;

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;  // JSON Schema
}

export interface ToolContext {
  cwd: string;
}

export interface ToolResult {
  content: string;
  isError?: boolean;
}

export interface Tool extends ToolDefinition {
  execute: (args: Record<string, unknown>, context: ToolContext) => Promise<ToolResult>;
}
