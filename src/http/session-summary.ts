import type { SessionRecord } from '../runtime/store.js';

export function projectSessionSummary(session: SessionRecord) {
  return {
    id: session.id,
    agent: session.agent,
    cwd: session.cwd,
    controlVersion: session.controlVersion,
    controlConfig: {
      provider: session.controlConfig.provider,
      model: session.controlConfig.model,
      maxTokens: session.controlConfig.maxTokens,
      contextWindow: session.controlConfig.contextWindow,
      baseURL: session.controlConfig.baseURL,
    },
    checkpointCount: session.checkpoints.length,
    createdAt: session.createdAt,
  };
}
