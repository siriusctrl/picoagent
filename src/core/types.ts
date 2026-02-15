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
  writeRoot?: string;  // if set, write_file restricts paths to this directory

  /**
   * Optional sandbox settings for tools that execute code (e.g. shell).
   * Intended to constrain writes to writeRoot/cwd for subagents.
   */
  sandbox?: {
    enabled?: boolean; // default: true for workers when writeRoot is set
    bwrapPath?: string;
    /** Hide /home and /root using tmpfs to reduce credential leakage. Default true. */
    hideHome?: boolean;
  };

  onTaskCreated?: (taskDir: string) => void;
  onSteer?: (taskId: string, message: string) => void;
  onAbort?: (taskId: string) => void;
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
