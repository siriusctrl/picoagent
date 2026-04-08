import type { z } from 'zod';
import type { AgentEnvironment } from './environment.js';

export interface TextContent {
  type: 'text';
  text: string;
}

export interface ToolCall {
  type: 'toolCall';
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export interface UserMessage {
  role: 'user';
  content: string;
}

export interface AssistantMessage {
  role: 'assistant';
  content: Array<TextContent | ToolCall>;
}

export interface ToolResultMessage {
  role: 'toolResult';
  toolCallId: string;
  content: string;
  isError: boolean;
}

export type Message = UserMessage | AssistantMessage | ToolResultMessage;

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
}

export type SessionModeId = 'ask' | 'exec';

export type ToolKind = 'read' | 'edit' | 'search' | 'execute' | 'other';

export interface ToolLocation {
  path: string;
  line?: number;
}

export interface ToolOutputText {
  type: 'text';
  text: string;
}

export interface ToolOutputDiff {
  type: 'diff';
  path: string;
  oldText?: string;
  newText: string;
}

export interface ToolOutputTerminal {
  type: 'terminal';
  terminalId: string;
}

export type ToolOutput = ToolOutputText | ToolOutputDiff | ToolOutputTerminal;

export interface ToolContext {
  sessionId: string;
  cwd: string;
  roots: string[];
  controlRoot: string;
  mode: SessionModeId;
  signal: AbortSignal;
  environment: AgentEnvironment;
}

export interface ToolResult {
  content: string;
  display?: ToolOutput[];
  rawOutput?: unknown;
  title?: string;
  locations?: ToolLocation[];
  kind?: ToolKind;
  isError?: boolean;
}

export interface ExecutedToolResult {
  message: ToolResultMessage;
  display?: ToolOutput[];
  rawOutput?: unknown;
  title: string;
  locations: ToolLocation[];
  kind: ToolKind;
}

export interface Tool<T extends z.ZodType = z.ZodType> {
  name: string;
  description: string;
  kind: ToolKind;
  parameters: T;
  title?: string | ((args: z.infer<T>, context: ToolContext) => string);
  locations?: (args: z.infer<T>, context: ToolContext) => ToolLocation[];
  execute: (args: z.infer<T>, context: ToolContext) => Promise<ToolResult>;
}
