import type { AgentPresetId, Message } from '../core/types.ts';
import type {
  PendingRunEvent,
  RunRecord,
  RuntimeStore,
  SessionCompactResult,
  SessionRecord,
  SessionSnapshot,
  SessionStore,
} from './store.ts';

export class StoreBackedSessionStore implements SessionStore {
  constructor(private readonly store: RuntimeStore) {}

  async createSession(record: SessionRecord): Promise<SessionRecord> {
    return this.store.createSession(record);
  }

  async getSession(id: string): Promise<SessionRecord | undefined> {
    return this.store.getSession(id);
  }

  async createRun(record: RunRecord): Promise<void> {
    await this.store.createRun(record);
  }

  async updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): Promise<void> {
    await this.store.updateRun(runId, patch);
  }

  async appendRunEvent(runId: string, event: PendingRunEvent): Promise<void> {
    await this.store.appendRunEvent(runId, event);
  }

  async setSessionAgent(sessionId: string, agent: AgentPresetId): Promise<void> {
    await this.store.setSessionAgent(sessionId, agent);
  }

  async refreshSessionControl(
    sessionId: string,
    control: {
      controlVersion: string;
      controlConfig: SessionRecord['controlConfig'];
      systemPrompts: SessionRecord['systemPrompts'];
    },
  ): Promise<void> {
    await this.store.refreshSessionControl(sessionId, control);
  }

  async attachRunToSession(sessionId: string, runId: string): Promise<void> {
    await this.store.attachRunToSession(sessionId, runId);
  }

  async finishSessionRun(sessionId: string, runId: string, messages: Message[]): Promise<void> {
    await this.store.finishSessionRun(sessionId, runId, messages);
  }

  async clearSessionActiveRun(sessionId: string, runId: string): Promise<void> {
    await this.store.clearSessionActiveRun(sessionId, runId);
  }

  async getSessionSnapshot(sessionId: string): Promise<SessionSnapshot | undefined> {
    return this.store.getSessionSnapshot(sessionId);
  }

  async listSessionResources(sessionId: string, resourcePath?: string): Promise<string[] | undefined> {
    return this.store.listSessionResources(sessionId, resourcePath);
  }

  async readSessionResource(sessionId: string, resourcePath: string): Promise<string | undefined> {
    return this.store.readSessionResource(sessionId, resourcePath);
  }

  async compactSession(sessionId: string, keepLastMessages?: number): Promise<SessionCompactResult | undefined> {
    return this.store.compactSession(sessionId, keepLastMessages);
  }
}
