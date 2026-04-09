import { randomUUID } from 'node:crypto';
import http from 'node:http';
import { createAppBootstrap } from '../bootstrap/index.js';
import { runAgentLoop } from '../core/loop.js';
import { AgentPresetId, AssistantMessage, Message } from '../core/types.js';
import { buildSystemPrompt } from '../prompting/prompt.js';
import { LocalEnvironment } from './local-environment.js';
import { buildOpenApiDocument } from './openapi.js';

export interface HttpServerOptions {
  cwd?: string;
  hostname?: string;
  port?: number;
}

type RunStatus = 'running' | 'completed' | 'failed';

type RunEvent =
  | {
      type: 'run_started';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      agent: AgentPresetId;
      prompt: string;
    }
  | {
      type: 'assistant_delta';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      text: string;
    }
  | {
      type: 'tool_call';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      title: string;
      toolCallId: string;
      status: 'pending';
      kind?: string;
      rawInput?: unknown;
    }
  | {
      type: 'tool_call_update';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      toolCallId: string;
      title?: string;
      status?: string;
      rawOutput?: unknown;
      text?: string;
    }
  | {
      type: 'done';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      output: string;
    }
  | {
      type: 'error';
      index: number;
      timestamp: string;
      runId: string;
      sessionId?: string;
      message: string;
    };

interface RunState {
  id: string;
  sessionId?: string;
  agent: AgentPresetId;
  prompt: string;
  status: RunStatus;
  output: string;
  error?: string;
  createdAt: string;
  startedAt?: string;
  finishedAt?: string;
  events: RunEvent[];
  listeners: Set<(event: RunEvent) => void>;
  controller?: AbortController;
}

interface SessionState {
  id: string;
  cwd: string;
  roots: string[];
  agent: AgentPresetId;
  createdAt: string;
  activeRunId?: string;
  runIds: string[];
  messages: Message[];
}

interface RunSnapshot {
  id: string;
  sessionId?: string;
  agent: AgentPresetId;
  status: RunStatus;
  prompt: string;
  output: string;
  error?: string;
  createdAt: string;
  startedAt?: string;
  finishedAt?: string;
}

interface SessionSnapshot {
  id: string;
  cwd: string;
  agent: AgentPresetId;
  createdAt: string;
  activeRunId?: string;
  runs: RunSnapshot[];
}

class NotFoundError extends Error {}
class ConflictError extends Error {}
class ValidationError extends Error {}

function nowIso(): string {
  return new Date().toISOString();
}

function assistantText(message: AssistantMessage): string {
  return message.content
    .filter((item): item is { type: 'text'; text: string } => item.type === 'text')
    .map((item) => item.text)
    .join('');
}

async function readJsonBody(request: http.IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }

  if (chunks.length === 0) {
    return {};
  }

  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch (error: unknown) {
    throw new ValidationError(
      error instanceof Error ? `Invalid JSON body: ${error.message}` : 'Invalid JSON body',
    );
  }
}

function sendJson(response: http.ServerResponse, status: number, payload: unknown): void {
  response.statusCode = status;
  response.setHeader('content-type', 'application/json; charset=utf-8');
  response.end(JSON.stringify(payload));
}

function sendSse(response: http.ServerResponse, event: RunEvent): void {
  response.write(`event: ${event.type}\n`);
  response.write(`data: ${JSON.stringify(event)}\n\n`);
}

function wantsSse(request: http.IncomingMessage): boolean {
  return request.headers.accept?.includes('text/event-stream') ?? false;
}

function parseAgent(value: unknown, fallback: AgentPresetId = 'ask'): AgentPresetId {
  if (value === undefined) {
    return fallback;
  }

  if (value === 'ask' || value === 'exec') {
    return value;
  }

  throw new ValidationError(`Unsupported agent: ${String(value)}`);
}

function requireAgent(value: unknown): AgentPresetId {
  if (value === undefined) {
    throw new ValidationError('agent is required');
  }

  return parseAgent(value);
}

function requirePrompt(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) {
    throw new ValidationError('prompt is required');
  }

  return value;
}

class HttpRuntimeService {
  private readonly bootstrap;
  private readonly environment = new LocalEnvironment();
  private readonly sessions = new Map<string, SessionState>();
  private readonly runs = new Map<string, RunState>();

  constructor(private readonly cwd: string) {
    this.bootstrap = createAppBootstrap(cwd);
  }

  createSession(agent: AgentPresetId = 'ask'): SessionState {
    const session: SessionState = {
      id: randomUUID(),
      cwd: this.cwd,
      roots: [this.cwd],
      agent,
      createdAt: nowIso(),
      runIds: [],
      messages: [],
    };
    this.sessions.set(session.id, session);
    return session;
  }

  getSession(id: string): SessionState {
    const session = this.sessions.get(id);
    if (!session) {
      throw new NotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  getRun(id: string): RunState {
    const run = this.runs.get(id);
    if (!run) {
      throw new NotFoundError(`Run ${id} not found`);
    }

    return run;
  }

  getRunSnapshot(id: string): RunSnapshot {
    return this.toRunSnapshot(this.getRun(id));
  }

  getSessionSnapshot(id: string): SessionSnapshot {
    const session = this.getSession(id);
    return {
      id: session.id,
      cwd: session.cwd,
      agent: session.agent,
      createdAt: session.createdAt,
      activeRunId: session.activeRunId,
      runs: session.runIds
        .map((runId) => this.runs.get(runId))
        .filter((run): run is RunState => Boolean(run))
        .map((run) => this.toRunSnapshot(run)),
    };
  }

  createStandaloneRun(prompt: string, agent: AgentPresetId): RunSnapshot {
    const run = this.createRun(prompt, agent);
    this.startRun(run);
    return this.toRunSnapshot(run);
  }

  createSessionRun(sessionId: string, prompt: string, agent?: AgentPresetId): RunSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    const run = this.createRun(prompt, agent ?? session.agent, session.id);
    session.activeRunId = run.id;
    session.runIds.push(run.id);
    this.startRun(run, session);
    return this.toRunSnapshot(run);
  }

  setSessionAgent(sessionId: string, agent: AgentPresetId): SessionSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    session.agent = agent;
    return this.getSessionSnapshot(sessionId);
  }

  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } {
    const run = this.getRun(runId);
    return {
      runId,
      status: run.status,
      events: [...run.events],
    };
  }

  subscribeToRun(runId: string, listener: (event: RunEvent) => void): () => void {
    const run = this.getRun(runId);
    run.listeners.add(listener);
    for (const event of run.events) {
      listener(event);
    }

    return () => {
      run.listeners.delete(listener);
    };
  }

  private createRun(prompt: string, agent: AgentPresetId, sessionId?: string): RunState {
    const run: RunState = {
      id: randomUUID(),
      sessionId,
      agent,
      prompt,
      status: 'running',
      output: '',
      createdAt: nowIso(),
      events: [],
      listeners: new Set(),
    };
    this.runs.set(run.id, run);
    return run;
  }

  private toRunSnapshot(run: RunState): RunSnapshot {
    return {
      id: run.id,
      sessionId: run.sessionId,
      agent: run.agent,
      status: run.status,
      prompt: run.prompt,
      output: run.output,
      error: run.error,
      createdAt: run.createdAt,
      startedAt: run.startedAt,
      finishedAt: run.finishedAt,
    };
  }

  private emit(run: RunState, event: { type: RunEvent['type'] } & Record<string, unknown>): void {
    const record = {
      ...event,
      index: run.events.length,
      timestamp: nowIso(),
      runId: run.id,
      sessionId: run.sessionId,
    } as RunEvent;
    run.events.push(record);

    for (const listener of run.listeners) {
      listener(record);
    }
  }

  private startRun(run: RunState, session?: SessionState): void {
    void this.executeRun(run, session).catch(() => {
      // Run failures are captured in the run state and emitted as events.
    });
  }

  private async executeRun(run: RunState, session?: SessionState): Promise<void> {
    const controller = new AbortController();
    run.controller = controller;
    run.startedAt = nowIso();
    this.emit(run, {
      type: 'run_started',
      agent: run.agent,
      prompt: run.prompt,
    });

    const conversation: Message[] = session
      ? [...session.messages, { role: 'user', content: run.prompt }]
      : [{ role: 'user', content: run.prompt }];

    const tools = this.bootstrap.registry.forAgent(run.agent);
    const systemPrompt = buildSystemPrompt(this.bootstrap.controlDir, run.agent, tools);

    try {
      const finalMessage = await runAgentLoop(
        conversation,
        tools,
        this.bootstrap.provider,
        {
          sessionId: run.id,
          cwd: session?.cwd ?? this.cwd,
          roots: session?.roots ?? [this.cwd],
          controlRoot: this.bootstrap.controlDir,
          agent: run.agent,
          signal: controller.signal,
          environment: this.environment,
        },
        systemPrompt,
        {
          onTextDelta: async (text) => {
            run.output += text;
            this.emit(run, {
              type: 'assistant_delta',
              text,
            });
          },
          onToolStart: async (toolCall, tool) => {
            this.emit(run, {
              type: 'tool_call',
              title: tool?.title && typeof tool.title === 'string' ? tool.title : tool?.name ?? toolCall.name,
              toolCallId: toolCall.id,
              status: 'pending',
              kind: tool?.kind,
              rawInput: toolCall.arguments,
            });
          },
          onToolEnd: async (toolCall, _tool, result) => {
            this.emit(run, {
              type: 'tool_call_update',
              toolCallId: toolCall.id,
              title: result.title,
              status: result.message.isError ? 'failed' : 'completed',
              rawOutput: result.rawOutput,
              text: result.message.content,
            });
          },
        },
      );

      run.output = assistantText(finalMessage);
      run.status = 'completed';
      run.finishedAt = nowIso();
      this.emit(run, {
        type: 'done',
        output: run.output,
      });

      if (session) {
        session.messages = conversation;
      }
    } catch (error: unknown) {
      run.status = 'failed';
      run.error = error instanceof Error ? error.message : String(error);
      run.finishedAt = nowIso();
      this.emit(run, {
        type: 'error',
        message: run.error,
      });
    } finally {
      run.controller = undefined;
      if (session?.activeRunId === run.id) {
        session.activeRunId = undefined;
      }
    }
  }
}

export async function startHttpServer(options: HttpServerOptions = {}): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4096;
  const service = new HttpRuntimeService(options.cwd ?? process.cwd());

  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? '/', `http://${request.headers.host ?? `${hostname}:${port}`}`);
      const { pathname } = url;

      if (request.method === 'GET' && pathname === '/openapi.json') {
        sendJson(response, 200, buildOpenApiDocument());
        return;
      }

      if (request.method === 'POST' && pathname === '/runs') {
        const body = (await readJsonBody(request)) as { prompt?: unknown; agent?: unknown };
        const run = service.createStandaloneRun(requirePrompt(body.prompt), parseAgent(body.agent));
        sendJson(response, 202, { runId: run.id, status: run.status });
        return;
      }

      const runMatch = pathname.match(/^\/runs\/([^/]+)$/);
      if (request.method === 'GET' && runMatch) {
        sendJson(response, 200, service.getRunSnapshot(runMatch[1]));
        return;
      }

      const eventsMatch = pathname.match(/^\/events\/([^/]+)$/);
      if (request.method === 'GET' && eventsMatch) {
        const runId = eventsMatch[1];
        if (!wantsSse(request)) {
          sendJson(response, 200, service.getRunEvents(runId));
          return;
        }

        service.getRunSnapshot(runId);

        let ended = false;
        const endStream = () => {
          if (ended) {
            return;
          }

          ended = true;
          if (!response.writableEnded) {
            response.end();
          }
        };

        response.writeHead(200, {
          'content-type': 'text/event-stream; charset=utf-8',
          'cache-control': 'no-cache, no-transform',
          connection: 'keep-alive',
        });

        const keepAlive = setInterval(() => {
          response.write(': keep-alive\n\n');
        }, 15000);

        let unsubscribe = () => {};
        unsubscribe = service.subscribeToRun(runId, (event) => {
          sendSse(response, event);
          if (event.type === 'done' || event.type === 'error') {
            clearInterval(keepAlive);
            unsubscribe();
            endStream();
          }
        });

        request.on('close', () => {
          clearInterval(keepAlive);
          unsubscribe();
          endStream();
        });
        return;
      }

      if (request.method === 'POST' && pathname === '/sessions') {
        const body = (await readJsonBody(request)) as { agent?: unknown };
        const session = service.createSession(parseAgent(body.agent));
        sendJson(response, 201, {
          id: session.id,
          agent: session.agent,
          cwd: session.cwd,
          createdAt: session.createdAt,
        });
        return;
      }

      const sessionMatch = pathname.match(/^\/sessions\/([^/]+)$/);
      if (request.method === 'GET' && sessionMatch) {
        sendJson(response, 200, service.getSessionSnapshot(sessionMatch[1]));
        return;
      }

      const sessionRunMatch = pathname.match(/^\/sessions\/([^/]+)\/runs$/);
      if (request.method === 'POST' && sessionRunMatch) {
        const body = (await readJsonBody(request)) as { prompt?: unknown; agent?: unknown };
        const run = service.createSessionRun(
          sessionRunMatch[1],
          requirePrompt(body.prompt),
          body.agent === undefined ? undefined : parseAgent(body.agent),
        );
        sendJson(response, 202, { runId: run.id, status: run.status, sessionId: run.sessionId });
        return;
      }

      const sessionAgentMatch = pathname.match(/^\/sessions\/([^/]+)\/agent$/);
      if (request.method === 'POST' && sessionAgentMatch) {
        const body = (await readJsonBody(request)) as { agent?: unknown };
        sendJson(response, 200, service.setSessionAgent(sessionAgentMatch[1], requireAgent(body.agent)));
        return;
      }

      sendJson(response, 404, { error: 'not found' });
    } catch (error: unknown) {
      if (response.headersSent) {
        if (!response.writableEnded) {
          response.end();
        }
        return;
      }

      if (error instanceof NotFoundError) {
        sendJson(response, 404, { error: error.message });
        return;
      }

      if (error instanceof ConflictError) {
        sendJson(response, 409, { error: error.message });
        return;
      }

      if (error instanceof ValidationError) {
        sendJson(response, 400, { error: error.message });
        return;
      }

      sendJson(response, 500, { error: error instanceof Error ? error.message : String(error) });
    }
  });

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, hostname, () => {
      server.off('error', reject);
      resolve();
    });
  });

  return server;
}
