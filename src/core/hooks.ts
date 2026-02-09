import { AssistantMessage, Message, ToolCall, ToolResultMessage } from "./types.js";

export interface AgentHooks {
  onLoopStart?(): void | Promise<void>;
  onLoopEnd?(turns: number): void | Promise<void>;
  onLlmStart?(messages: Message[]): void | Promise<void>;
  onLlmEnd?(response: AssistantMessage, durationMs: number): void | Promise<void>;
  onToolStart?(call: ToolCall): void | Promise<void>;
  onToolEnd?(call: ToolCall, result: ToolResultMessage, durationMs: number): ToolResultMessage | void | Promise<ToolResultMessage | void>;
  onTurnEnd?(messages: Message[]): void | Promise<void>;
  onTextDelta?(text: string): void;
  onError?(error: Error): void | Promise<void>;
}

/** Combine multiple hooks. For onToolEnd, chain result modifications. */
export function combineHooks(...hookSets: (AgentHooks | undefined)[]): AgentHooks {
  const filtered = hookSets.filter((h): h is AgentHooks => h !== undefined);
  if (filtered.length === 0) return {};
  if (filtered.length === 1) return filtered[0];
  
  const combined: AgentHooks = {};
  const hookNames: (keyof AgentHooks)[] = [
    "onLoopStart", "onLoopEnd", "onLlmStart", "onLlmEnd",
    "onToolStart", "onToolEnd", "onTurnEnd", "onTextDelta", "onError"
  ];
  
  for (const name of hookNames) {
    const handlers = filtered.map(h => h[name]).filter(Boolean);
    if (handlers.length === 0) continue;
    
    if (name === "onToolEnd") {
      // Chain: each handler can modify the result
      (combined as any)[name] = async (call: any, result: any, dur: any) => {
        let current = result;
        for (const handler of handlers) {
          const modified = await (handler as any)(call, current, dur);
          if (modified) current = modified;
        }
        return current;
      };
    } else if (name === "onTextDelta") {
      (combined as any)[name] = (text: string) => {
        for (const handler of handlers) (handler as any)(text);
      };
    } else {
      (combined as any)[name] = async (...args: any[]) => {
        for (const handler of handlers) await (handler as any)(...args);
      };
    }
  }
  return combined;
}
