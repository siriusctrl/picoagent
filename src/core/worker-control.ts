import { AgentHooks } from "./hooks.js";
import { Message } from "./types.js";

export class WorkerControl {
  private _aborted = false;
  private steerQueue: string[] = [];
  
  get aborted(): boolean { return this._aborted; }
  abort(): void { this._aborted = true; }
  steer(message: string): void { this.steerQueue.push(message); }
  consumeSteer(): string | undefined { return this.steerQueue.shift(); }
}

export class AbortError extends Error {
  constructor(taskId: string) {
    super(`Task ${taskId} was aborted`);
    this.name = "AbortError";
  }
}

export function createWorkerControlHooks(control: WorkerControl, taskId: string): AgentHooks {
  return {
    onToolEnd(_call, result) {
      if (control.aborted) throw new AbortError(taskId);
      return result;
    },
    onTurnEnd(messages: Message[]) {
      let msg: string | undefined;
      while ((msg = control.consumeSteer()) !== undefined) {
        messages.push({ role: "user", content: `[Steer] ${msg}` });
      }
    }
  };
}
