import type { z } from "zod";

// === Internal message types (plain interfaces, no runtime validation) ===

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

// === Tool definition (JSON Schema version, for Provider interface) ===

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;  // JSON Schema
}

// === Tool (Zod version, for agent loop) ===

export interface ToolContext {
  cwd: string;
  tasksRoot: string;
  onTaskCreated?: (taskDir: string) => void;
}

export interface ToolResult {
  content: string;
  isError?: boolean;
}

export interface Tool<T extends z.ZodType = z.ZodType> {
  name: string;
  description: string;
  parameters: T;
  execute: (args: z.infer<T>, context: ToolContext) => Promise<ToolResult>;
}
