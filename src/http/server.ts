import { randomUUID } from 'node:crypto';
import http from 'node:http';
import { buildSessionControlSnapshot, computeControlVersion, SessionControlSnapshot } from '../bootstrap/control-snapshot.js';
import { createAppBootstrap } from '../bootstrap/index.js';
import type { AgentEnvironment } from '../core/environment.js';
import { runAgentLoop } from '../core/loop.js';
import { AgentPresetId, AssistantMessage, Message } from '../core/types.js';
import { createProvider } from '../providers/index.js';
import { LocalEnvironment } from './local-environment.js';
import { buildOpenApiDocument } from './openapi.js';
import {
  EmittedRunEvent,
  InMemoryRuntimeStore,
  RunEvent,
  RunSnapshot,
  RunStatus,
  SessionRecord,
  SessionSnapshot,
} from './runtime-store.js';

export interface HttpServerOptions {
  cwd?: string;
  hostname?: string;
  port?: number;
  environment?: AgentEnvironment;
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
  private readonly store = new InMemoryRuntimeStore();

  constructor(
    private readonly cwd: string,
    private readonly environment: AgentEnvironment,
  ) {
    this.bootstrap = createAppBootstrap(cwd);
  }

  createSession(agent: AgentPresetId = 'ask'): SessionRecord {
    const control = buildSessionControlSnapshot(this.cwd, this.bootstrap.registry);
    const session: SessionRecord = {
      id: randomUUID(),
      cwd: this.cwd,
      roots: [this.cwd],
      agent,
      controlVersion: control.controlVersion,
      controlConfig: control.config,
      systemPrompts: control.systemPrompts,
      createdAt: nowIso(),
      runIds: [],
      messages: [],
    };
    return this.store.createSession(session);
  }

  getSession(id: string): SessionRecord {
    const session = this.store.getSession(id);
    if (!session) {
      throw new NotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  getRun(id: string) {
    const run = this.store.getRun(id);
    if (!run) {
      throw new NotFoundError(`Run ${id} not found`);
    }

    return run;
  }

  getRunSnapshot(id: string): RunSnapshot {
    const run = this.store.getRunSnapshot(id);
    if (!run) {
      throw new NotFoundError(`Run ${id} not found`);
    }

    return run;
  }

  getSessionSnapshot(id: string): SessionSnapshot {
    const session = this.store.getSessionSnapshot(id);
    if (!session) {
      throw new NotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  createStandaloneRun(prompt: string, agent: AgentPresetId): RunSnapshot {
    const control = buildSessionControlSnapshot(this.cwd, this.bootstrap.registry);
    const run = this.createRun(prompt, agent);
    this.startRun(run, control);
    return this.getRunSnapshot(run.id);
  }

  createSessionRun(sessionId: string, prompt: string, agent?: AgentPresetId): RunSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    const control = this.ensureSessionControlSnapshot(session);
    const run = this.createRun(prompt, agent ?? session.agent, session.id);
    this.store.attachRunToSession(session.id, run.id);
    this.startRun(run, control, session);
    return this.getRunSnapshot(run.id);
  }

  setSessionAgent(sessionId: string, agent: AgentPresetId): SessionSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    this.store.setSessionAgent(sessionId, agent);
    return this.getSessionSnapshot(sessionId);
  }

  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } {
    const events = this.store.getRunEvents(runId);
    if (!events) {
      throw new NotFoundError(`Run ${runId} not found`);
    }

    return events;
  }

  subscribeToRun(runId: string, listener: (event: RunEvent) => void): () => void {
    const unsubscribe = this.store.subscribeToRun(runId, listener);
    if (!unsubscribe) {
      throw new NotFoundError(`Run ${runId} not found`);
    }

    return unsubscribe;
  }

  private createRun(prompt: string, agent: AgentPresetId, sessionId?: string) {
    return this.store.createRun({
      id: randomUUID(),
      sessionId,
      agent,
      prompt,
      status: 'running',
      output: '',
      createdAt: nowIso(),
      events: [],
    });
  }

  private emit(runId: string, event: EmittedRunEvent): void {
    this.store.appendRunEvent(runId, {
      ...event,
      timestamp: nowIso(),
    });
  }

  private ensureSessionControlSnapshot(session: SessionRecord): SessionControlSnapshot {
    const latestVersion = computeControlVersion(session.cwd);
    if (latestVersion === session.controlVersion) {
      return {
        workspaceRoot: session.cwd,
        controlVersion: session.controlVersion,
        config: session.controlConfig,
        systemPrompts: session.systemPrompts,
      };
    }

    const refreshed = buildSessionControlSnapshot(session.cwd, this.bootstrap.registry, latestVersion);
    this.store.refreshSessionControl(session.id, {
      controlVersion: refreshed.controlVersion,
      controlConfig: refreshed.config,
      systemPrompts: refreshed.systemPrompts,
    });
    return refreshed;
  }

  private startRun(
    run: ReturnType<HttpRuntimeService['createRun']>,
    control: SessionControlSnapshot,
    session?: SessionRecord,
  ): void {
    void this.executeRun(run, control, session).catch(() => {
      // Run failures are captured in the run state and emitted as events.
    });
  }

  private async executeRun(
    run: ReturnType<HttpRuntimeService['createRun']>,
    control: SessionControlSnapshot,
    session?: SessionRecord,
  ): Promise<void> {
    const controller = new AbortController();
    const startedAt = nowIso();
    this.store.updateRun(run.id, { startedAt });
    this.emit(run.id, {
      type: 'run_started',
      runId: run.id,
      sessionId: run.sessionId,
      agent: run.agent,
      prompt: run.prompt,
    });

    const conversation: Message[] = session
      ? [...session.messages, { role: 'user', content: run.prompt }]
      : [{ role: 'user', content: run.prompt }];

    const tools = this.bootstrap.registry.forAgent(run.agent);
    const systemPrompt = control.systemPrompts[run.agent];
    const provider = createProvider(control.config);

    try {
      const finalMessage = await runAgentLoop(
        conversation,
        tools,
        provider,
        {
          sessionId: run.id,
          cwd: session?.cwd ?? this.cwd,
          roots: session?.roots ?? [this.cwd],
          controlRoot: control.workspaceRoot,
          agent: run.agent,
          signal: controller.signal,
          environment: this.environment,
        },
        systemPrompt,
        {
          onTextDelta: async (text) => {
            const latestRun = this.getRun(run.id);
            this.store.updateRun(run.id, { output: latestRun.output + text });
            this.emit(run.id, {
              type: 'assistant_delta',
              runId: run.id,
              sessionId: run.sessionId,
              text,
            });
          },
          onToolStart: async (toolCall, tool) => {
            this.emit(run.id, {
              type: 'tool_call',
              runId: run.id,
              sessionId: run.sessionId,
              title: tool?.title && typeof tool.title === 'string' ? tool.title : tool?.name ?? toolCall.name,
              toolCallId: toolCall.id,
              status: 'pending',
              kind: tool?.kind,
              rawInput: toolCall.arguments,
            });
          },
          onToolEnd: async (toolCall, _tool, result) => {
            this.emit(run.id, {
              type: 'tool_call_update',
              runId: run.id,
              sessionId: run.sessionId,
              toolCallId: toolCall.id,
              title: result.title,
              status: result.message.isError ? 'failed' : 'completed',
              rawOutput: result.rawOutput,
              text: result.message.content,
            });
          },
        },
      );

      const finishedAt = nowIso();
      this.store.updateRun(run.id, {
        output: assistantText(finalMessage),
        status: 'completed',
        finishedAt,
      });
      this.emit(run.id, {
        type: 'done',
        runId: run.id,
        sessionId: run.sessionId,
        output: assistantText(finalMessage),
      });

      if (session) {
        this.store.finishSessionRun(session.id, run.id, conversation);
      }
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      this.store.updateRun(run.id, {
        status: 'failed',
        error: message,
        finishedAt: nowIso(),
      });
      this.emit(run.id, {
        type: 'error',
        runId: run.id,
        sessionId: run.sessionId,
        message,
      });
    } finally {
      if (session) {
        this.store.clearSessionActiveRun(session.id, run.id);
      }
    }
  }
}

export async function startHttpServer(options: HttpServerOptions = {}): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4096;
  const service = new HttpRuntimeService(options.cwd ?? process.cwd(), options.environment ?? new LocalEnvironment());

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
          controlVersion: session.controlVersion,
          controlConfig: {
            provider: session.controlConfig.provider,
            model: session.controlConfig.model,
            maxTokens: session.controlConfig.maxTokens,
            contextWindow: session.controlConfig.contextWindow,
            baseURL: session.controlConfig.baseURL,
          },
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
