import type { AgentPresetId, Message } from '../core/types.js';
import type {
  PendingRunEvent,
  RunRecord,
  SessionCompactResult,
  SessionRecord,
  SessionSnapshot,
  SessionStore,
} from './store.js';

async function parseJsonResponse<T>(response: Response): Promise<T> {
  if (response.status === 404) {
    return undefined as T;
  }

  if (!response.ok) {
    let message = `Request failed with ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: unknown };
      if (typeof payload.error === 'string') {
        message = payload.error;
      }
    } catch {
      // Ignore malformed error payloads.
    }

    throw new Error(message);
  }

  return response.json() as Promise<T>;
}

export class HttpSessionStore implements SessionStore {
  constructor(private readonly baseUrl: string) {}

  private url(path: string): string {
    return `${this.baseUrl.replace(/\/+$/, '')}${path}`;
  }

  async createSession(record: SessionRecord): Promise<SessionRecord> {
    const response = await fetch(this.url('/_store/sessions'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(record),
    });
    return parseJsonResponse<SessionRecord>(response);
  }

  async getSession(id: string): Promise<SessionRecord | undefined> {
    const response = await fetch(this.url(`/_store/sessions/${id}`));
    return parseJsonResponse<SessionRecord | undefined>(response);
  }

  async createRun(record: RunRecord): Promise<void> {
    const response = await fetch(this.url('/_store/runs'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(record),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async updateRun(runId: string, patch: Partial<Omit<RunRecord, 'id' | 'events'>>): Promise<void> {
    const response = await fetch(this.url(`/_store/runs/${runId}`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(patch),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async appendRunEvent(runId: string, event: PendingRunEvent): Promise<void> {
    const response = await fetch(this.url(`/_store/runs/${runId}/events`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(event),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async setSessionAgent(sessionId: string, agent: AgentPresetId): Promise<void> {
    const response = await fetch(this.url(`/sessions/${sessionId}/agent`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ agent }),
    });
    await parseJsonResponse<SessionSnapshot>(response);
  }

  async refreshSessionControl(
    sessionId: string,
    control: {
      controlVersion: SessionRecord['controlVersion'];
      controlConfig: SessionRecord['controlConfig'];
      systemPrompts: SessionRecord['systemPrompts'];
    },
  ): Promise<void> {
    const response = await fetch(this.url(`/_store/sessions/${sessionId}/control`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(control),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async attachRunToSession(sessionId: string, runId: string): Promise<void> {
    const response = await fetch(this.url(`/_store/sessions/${sessionId}/attach-run`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ runId }),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async finishSessionRun(sessionId: string, runId: string, messages: Message[]): Promise<void> {
    const response = await fetch(this.url(`/_store/sessions/${sessionId}/finish-run`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ runId, messages }),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async clearSessionActiveRun(sessionId: string, runId: string): Promise<void> {
    const response = await fetch(this.url(`/_store/sessions/${sessionId}/clear-active-run`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ runId }),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async getSessionSnapshot(sessionId: string): Promise<SessionSnapshot | undefined> {
    const response = await fetch(this.url(`/sessions/${sessionId}`));
    return parseJsonResponse<SessionSnapshot | undefined>(response);
  }

  async listSessionResources(sessionId: string, resourcePath?: string): Promise<string[] | undefined> {
    const query = resourcePath ? `?path=${encodeURIComponent(resourcePath)}` : '';
    const response = await fetch(this.url(`/sessions/${sessionId}/resources${query}`));
    const payload = await parseJsonResponse<{ entries: string[] } | undefined>(response);
    return payload?.entries;
  }

  async readSessionResource(sessionId: string, resourcePath: string): Promise<string | undefined> {
    const response = await fetch(this.url(`/sessions/${sessionId}/resources/${encodeURIComponent(resourcePath)}`));
    if (response.status === 404) {
      return undefined;
    }
    if (!response.ok) {
      throw new Error(`Request failed with ${response.status}`);
    }
    return response.text();
  }

  async compactSession(sessionId: string, keepLastMessages?: number): Promise<SessionCompactResult | undefined> {
    const response = await fetch(this.url(`/sessions/${sessionId}/compact`), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(keepLastMessages === undefined ? {} : { keepLastMessages }),
    });
    const payload = await parseJsonResponse<{ checkpoint: SessionCompactResult } | undefined>(response);
    return payload?.checkpoint;
  }
}
