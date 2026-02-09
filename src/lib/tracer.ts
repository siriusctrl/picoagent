import { appendFileSync, mkdirSync } from "fs";
import { join } from "path";
import { randomUUID } from "crypto";

export interface TraceEvent {
  trace_id: string;
  span_id: string;
  parent_span?: string;
  timestamp: string;  // ISO 8601
  event: string;      // "llm_start" | "llm_end" | "tool_start" | "tool_end" | "agent_start" | "agent_end" | "error"
  data?: Record<string, unknown>;
  duration_ms?: number;
}

export class Tracer {
  readonly traceId: string;
  private dir: string;
  private filePath: string;

  constructor(traceDir: string, traceId?: string) {
    this.traceId = traceId || randomUUID();
    this.dir = traceDir;
    mkdirSync(this.dir, { recursive: true });
    this.filePath = join(this.dir, `${this.traceId}.jsonl`);
  }

  span(parentSpan?: string): string {
    return randomUUID();
  }

  emit(event: Omit<TraceEvent, "trace_id" | "timestamp">): void {
    const line: TraceEvent = {
      ...event,
      trace_id: this.traceId,
      timestamp: new Date().toISOString()
    };
    appendFileSync(this.filePath, JSON.stringify(line) + "\n");
  }
}
