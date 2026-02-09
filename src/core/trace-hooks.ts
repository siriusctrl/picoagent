import { Tracer } from "./trace.js";
import { AgentHooks } from "./hooks.js";

export function createTraceHooks(tracer: Tracer, modelName?: string): AgentHooks {
  let agentSpanId: string;
  const toolSpans = new Map<string, string>(); // toolCallId -> spanId
  let currentLlmSpanId: string | undefined;
  
  return {
    onLoopStart() {
      agentSpanId = tracer.span();
      tracer.emit({ event: "agent_start", span_id: agentSpanId, data: { model: modelName } });
    },
    onLoopEnd(turns) {
      tracer.emit({ event: "agent_end", span_id: agentSpanId, data: { total_turns: turns } });
    },
    onLlmStart(messages) {
      currentLlmSpanId = tracer.span(agentSpanId);
      tracer.emit({ event: "llm_start", span_id: currentLlmSpanId, parent_span: agentSpanId, data: { message_count: messages.length } });
    },
    onLlmEnd(_response, durationMs) {
      if (currentLlmSpanId) {
        tracer.emit({ event: "llm_end", span_id: currentLlmSpanId, parent_span: agentSpanId, duration_ms: durationMs });
      }
    },
    onToolStart(call) {
      const toolSpanId = tracer.span(currentLlmSpanId);
      toolSpans.set(call.id, toolSpanId);
      tracer.emit({ event: "tool_start", span_id: toolSpanId, parent_span: currentLlmSpanId, data: { tool: call.name, args: call.arguments } });
    },
    onToolEnd(call, result, durationMs) {
      const toolSpanId = toolSpans.get(call.id);
      if (toolSpanId) {
        tracer.emit({ event: "tool_end", span_id: toolSpanId, parent_span: currentLlmSpanId, duration_ms: durationMs, data: { tool: call.name, result_length: result.content.length, isError: result.isError } });
        toolSpans.delete(call.id);
      }
    },
    onError(error) {
      tracer.emit({ event: "error", span_id: agentSpanId, data: { message: error.message } });
    }
  };
}
