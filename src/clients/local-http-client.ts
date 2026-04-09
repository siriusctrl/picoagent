import type http from 'node:http';
import { AgentPresetId } from '../core/types.js';
import { startHttpServer } from '../http/server.js';

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

function getServerUrl(server: http.Server): string {
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Expected an inet server address');
  }

  return `http://127.0.0.1:${address.port}`;
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
  private server?: http.Server;
  private serverUrl?: string;
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

    const response = await fetch(`${this.serverUrl}/sessions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ agent: this.agent }),
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

    const createResponse = await fetch(`${this.serverUrl}/sessions/${this.sessionId}/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ prompt: text }),
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

    const response = await fetch(`${this.serverUrl}/sessions/${this.sessionId}/agent`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ agent }),
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
    await new Promise<void>((resolve, reject) => {
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolve();
      });
    });
  }
}
