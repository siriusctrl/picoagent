import type { SessionRecord } from '../runtime/store.ts';

export function projectSessionSummary(session: SessionRecord) {
  return {
    id: session.id,
    cwd: session.cwd,
    checkpointCount: session.checkpoints.length,
    createdAt: session.createdAt,
  };
}
