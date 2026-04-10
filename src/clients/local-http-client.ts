import { hc } from 'hono/client';
import { AgentPresetId } from '../core/types.ts';
import type { LocalServerHandle } from '../http/bun-server.ts';
import { startHttpServer, type HttpAppType } from '../http/server.ts';

export type ClientEvent =
  | { type: 'status'; text: string }
  | { type: 'agent'; agent: AgentPresetId }
  | { type: 'assistant_delta'; text: string }
  | { type: 'tool_call'; toolCallId: string; title: string; status?: string; kind?: string; rawInput?: unknown }
  | { type: 'tool_call_update'; toolCallId: string; title?: string | null; status?: string | null; rawOutput?: unknown; text?: string }
  | { type: 'error'; text: string };

export interface LocalHttpClientOptions {
  cwd: string;
  onEvent: (event: ClientEvent) => void;
}

type LocalHttpClientApi = ReturnType<typeof hc<HttpAppType>>;
type MinimalTypedClientResponse<T> = {
  ok: boolean;
  status: number;
  json: () => Promise<T>;
};

type SessionClientRoutes = {
  runs: {
    $post: (args: {
      param: { sessionId: string };
      json: { prompt: string; agent?: AgentPresetId };
    }) => Promise<MinimalTypedClientResponse<{ runId: string; status: string; sessionId?: string }>>;
  };
  agent: {
    $post: (args: { param: { sessionId: string }; json: { agent: AgentPresetId } }) => Promise<MinimalTypedClientResponse<unknown>>;
  };
};

type RunEvent = {
  type: string;
  text?: string;
  title?: string;
  toolCallId?: string;
  status?: string;
  kind?: string;
  rawInput?: unknown;
  rawOutput?: unknown;
  output?: string;
  message?: string;
};

function getServerUrl(server: LocalServerHandle): string {
  return server.url.origin;
}

function parseSseFrame(frame: string): RunEvent | null {
  const lines = frame.split('\n');
  const dataLines: string[] = [];

  for (const line of lines) {
    if (!line || line.startsWith(':')) {
      continue;
    }

    if (line.startsWith('data:')) {
      dataLines.push(line.slice('data:'.length).trimStart());
    }
  }

  if (dataLines.length === 0) {
    return null;
  }

  return JSON.parse(dataLines.join('\n')) as RunEvent;
}

export class LocalHttpClient {
  private readonly cwd: string;
  private readonly onEvent: (event: ClientEvent) => void;
  private server?: LocalServerHandle;
  private serverUrl?: string;
  private api?: LocalHttpClientApi;
  private sessionId?: string;
  private agent: AgentPresetId = 'ask';

  constructor(options: LocalHttpClientOptions) {
    this.cwd = options.cwd;
    this.onEvent = options.onEvent;
  }

  async start(): Promise<void> {
    this.server = await startHttpServer({
      cwd: this.cwd,
      hostname: '127.0.0.1',
      port: 0,
    });
    this.serverUrl = getServerUrl(this.server);
    this.api = hc<HttpAppType>(this.serverUrl);

    if (!this.api) {
      throw new Error('HTTP api client is not initialized');
    }

    const response = await this.api.sessions.$post({
      json: { agent: this.agent },
    });
    if (!response.ok) {
      throw new Error(`Failed to create session: ${response.status}`);
    }

    const session = (await response.json()) as { id: string; agent: AgentPresetId };
    this.sessionId = session.id;
    this.agent = session.agent;
    this.onEvent({ type: 'agent', agent: this.agent });
    this.onEvent({ type: 'status', text: `Connected with ${this.agent} agent` });
  }

  async sendPrompt(text: string): Promise<void> {
    if (!this.serverUrl || !this.sessionId) {
      throw new Error('HTTP session is not ready');
    }

    if (!this.api) {
      throw new Error('HTTP api client is not initialized');
    }

    const sessionRoutes = this.api.sessions as unknown as Record<string, SessionClientRoutes>;
    const createResponse = await sessionRoutes[':sessionId'].runs.$post({
      param: { sessionId: this.sessionId },
      json: { prompt: text },
    });
    if (!createResponse.ok) {
      throw new Error(`Failed to create run: ${createResponse.status}`);
    }

    const created = (await createResponse.json()) as { runId: string };
    const eventsResponse = await fetch(`${this.serverUrl}/events/${created.runId}`, {
      headers: { accept: 'text/event-stream' },
    });
    if (!eventsResponse.ok) {
      throw new Error(`Failed to read events: ${eventsResponse.status}`);
    }

    const reader = eventsResponse.body?.getReader();
    if (!reader) {
      throw new Error('Expected event stream body');
    }

    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        return;
      }

      buffer += decoder.decode(value, { stream: true });
      let boundary = buffer.indexOf('\n\n');
      while (boundary >= 0) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const event = parseSseFrame(frame);
        if (!event) {
          boundary = buffer.indexOf('\n\n');
          continue;
        }

        switch (event.type) {
          case 'assistant_delta':
            this.onEvent({ type: 'assistant_delta', text: event.text ?? '' });
            break;
          case 'tool_call':
            this.onEvent({
              type: 'tool_call',
              toolCallId: event.toolCallId ?? '',
              title: event.title ?? 'tool',
              status: event.status,
              kind: event.kind,
              rawInput: event.rawInput,
            });
            break;
          case 'tool_call_update':
            this.onEvent({
              type: 'tool_call_update',
              toolCallId: event.toolCallId ?? '',
              title: event.title,
              status: event.status,
              rawOutput: event.rawOutput,
              text: event.text,
            });
            break;
          case 'error':
            this.onEvent({ type: 'error', text: event.message ?? 'Run failed' });
            throw new Error(event.message ?? 'Run failed');
          default:
            break;
        }

        if (event.type === 'done') {
          return;
        }

        boundary = buffer.indexOf('\n\n');
      }
    }
  }

  async setAgent(agent: AgentPresetId): Promise<void> {
    if (!this.serverUrl || !this.sessionId || agent === this.agent) {
      return;
    }

    if (!this.api) {
      throw new Error('HTTP api client is not initialized');
    }

    const sessionRoutes = this.api.sessions as unknown as Record<string, SessionClientRoutes>;
    const response = await sessionRoutes[':sessionId'].agent.$post({
      param: { sessionId: this.sessionId },
      json: { agent },
    });
    if (!response.ok) {
      throw new Error(`Failed to set agent: ${response.status}`);
    }

    this.agent = agent;
    this.onEvent({ type: 'agent', agent });
    this.onEvent({ type: 'status', text: `Switched to ${agent} agent` });
  }

  async stop(): Promise<void> {
    if (!this.server) {
      return;
    }

    const server = this.server;
    this.server = undefined;
    this.serverUrl = undefined;
    this.sessionId = undefined;
    this.api = undefined;
    await server.stop(true);
  }
}
