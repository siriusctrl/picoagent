import http from 'node:http';
import { createAdaptorServer } from '@hono/node-server';
import { $, OpenAPIHono } from '@hono/zod-openapi';
import type { ExecutionBackend } from '../core/execution.js';
import type { MutableFilesystem } from '../core/filesystem.js';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.js';
import { LocalExecutionBackend } from '../runtime/local-execution-backend.js';
import type { RunEvent } from '../runtime/store.js';
import { RuntimeService } from '../runtime/service.js';
import {
  buildOpenApiDocument,
  compactSessionRoute,
  createSessionRoute,
  createSessionRunRoute,
  createStandaloneRunRoute,
  getRunEventsRoute,
  getRunRoute,
  getSessionRoute,
  listSessionResourcesRoute,
  readSessionResourceRoute,
  setSessionAgentRoute,
} from './openapi.js';

export interface HttpServerOptions {
  cwd?: string;
  hostname?: string;
  port?: number;
  filesystem?: MutableFilesystem;
  executionBackend?: ExecutionBackend;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

export interface HttpAppOptions {
  cwd?: string;
  filesystem?: MutableFilesystem;
  executionBackend?: ExecutionBackend;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

function projectSessionSummary(session: Awaited<ReturnType<RuntimeService['createSession']>>) {
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

function errorStatus(error: unknown): 400 | 404 | 409 | 500 {
  const status = typeof (error as { status?: unknown } | undefined)?.status === 'number'
    ? (error as { status: number }).status
    : undefined;

  if (status === 400 || status === 404 || status === 409) {
    return status;
  }

  return 500;
}

export function createHttpApp(options: HttpAppOptions = {}) {
  const service = new RuntimeService({
    cwd: options.cwd,
    filesystem: options.filesystem ?? new LocalWorkspaceFileSystem(),
    executionBackend: options.executionBackend ?? new LocalExecutionBackend(),
    runtimeRoot: options.runtimeRoot,
    persistentRuntime: options.persistentRuntime,
  });

  const app = new OpenAPIHono({
    defaultHook: (result, c) => {
      if (!result.success) {
        return c.json({ error: result.error.issues[0]?.message ?? 'Invalid request' }, 400);
      }
    },
  });

  const streamRunEvents = (runId: string, request: Request): Response => {
    service.getRunSnapshot(runId);

    const encoder = new TextEncoder();
    let unsubscribe = () => {};
    let keepAlive: ReturnType<typeof setInterval> | undefined;
    let closed = false;
    let streamController: ReadableStreamDefaultController<Uint8Array> | undefined;

    const sendFrame = (frame: string) => {
      if (!streamController || closed) {
        return;
      }

      streamController.enqueue(encoder.encode(frame));
    };

    const close = () => {
      if (closed) {
        return;
      }

      closed = true;
      if (keepAlive) {
        clearInterval(keepAlive);
      }
      unsubscribe();
      request.signal.removeEventListener('abort', close);
      try {
        streamController?.close();
      } catch {
        // Ignore close races after the client disconnects.
      }
    };

    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        streamController = controller;
        unsubscribe = service.subscribeToRun(runId, (event) => {
          sendFrame(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`);
          if (event.type === 'done' || event.type === 'error') {
            close();
          }
        });

        keepAlive = setInterval(() => {
          sendFrame(': keep-alive\n\n');
        }, 15000);

        request.signal.addEventListener('abort', close);
      },
      cancel() {
        close();
      },
    });

    return new Response(stream, {
      status: 200,
      headers: {
        'content-type': 'text/event-stream; charset=utf-8',
        'cache-control': 'no-cache, no-transform',
        connection: 'keep-alive',
      },
    });
  };

  app.onError((error, c) => {
    return c.json({ error: error instanceof Error ? error.message : String(error) }, errorStatus(error));
  });

  const appWithNotFound = $(app.notFound((c) => c.json({ error: 'not found' }, 404)));

  const appWithSpec = $(appWithNotFound.get('/openapi', (c) => c.json(buildOpenApiDocument(appWithNotFound))));

  const appWithStandaloneRun = appWithSpec.openapi(createStandaloneRunRoute, async (c) => {
    const body = c.req.valid('json');
    const run = await service.createStandaloneRun(body.prompt, body.agent ?? 'ask');
    return c.json({ runId: run.id, status: run.status }, 202);
  });

  const appWithGetRun = appWithStandaloneRun.openapi(getRunRoute, (c) => {
    const { runId } = c.req.valid('param');
    return c.json(service.getRunSnapshot(runId), 200);
  });

  const appWithRunEvents = appWithGetRun.openapi(getRunEventsRoute, (c) => {
    const { runId } = c.req.valid('param');
    if (c.req.header('accept')?.includes('text/event-stream')) {
      return streamRunEvents(runId, c.req.raw);
    }

    return c.json(service.getRunEvents(runId), 200);
  });

  const appWithCreateSession = appWithRunEvents.openapi(createSessionRoute, async (c) => {
    const body = c.req.valid('json');
    const session = await service.createSession(body.agent ?? 'ask');
    return c.json(projectSessionSummary(session), 201);
  });

  const appWithGetSession = appWithCreateSession.openapi(getSessionRoute, (c) => {
    const { sessionId } = c.req.valid('param');
    return c.json(service.getSessionSnapshot(sessionId), 200);
  });

  const appWithCreateSessionRun = appWithGetSession.openapi(createSessionRunRoute, async (c) => {
    const { sessionId } = c.req.valid('param');
    const body = c.req.valid('json');
    const run = await service.createSessionRun(sessionId, body.prompt, body.agent);
    return c.json({ runId: run.id, status: run.status, sessionId: run.sessionId }, 202);
  });

  const appWithListResources = appWithCreateSessionRun.openapi(listSessionResourcesRoute, (c) => {
    const { sessionId } = c.req.valid('param');
    const query = c.req.valid('query');
    const resourcePath = query.path ?? '.';
    return c.json(
      {
        sessionId,
        path: resourcePath,
        entries: service.listSessionResources(sessionId, resourcePath),
      },
      200,
    );
  });

  const appWithReadResource = appWithListResources.openapi(readSessionResourceRoute, (c) => {
    const { sessionId, resourcePath } = c.req.valid('param');
    return c.text(service.readSessionResource(sessionId, resourcePath), 200, {
      'content-type': resourcePath.endsWith('.jsonl')
        ? 'application/x-ndjson; charset=utf-8'
        : 'text/plain; charset=utf-8',
    });
  });

  const appWithSetAgent = appWithReadResource.openapi(setSessionAgentRoute, (c) => {
    const { sessionId } = c.req.valid('param');
    const body = c.req.valid('json');
    return c.json(service.setSessionAgent(sessionId, body.agent), 200);
  });

  const finalApp = appWithSetAgent.openapi(compactSessionRoute, (c) => {
    const { sessionId } = c.req.valid('param');
    const body = c.req.valid('json');
    return c.json(service.compactSession(sessionId, body.keepLastMessages), 200);
  });

  return {
    app: finalApp,
    service,
  };
}

export type HttpAppType = ReturnType<typeof createHttpApp>['app'];

export async function startHttpServer(options: HttpServerOptions = {}): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4096;
  const { app } = createHttpApp(options);

  const server = createAdaptorServer({
    fetch: app.fetch,
    hostname,
    port,
  }) as unknown as http.Server;

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, hostname, () => {
      server.off('error', reject);
      resolve();
    });
  });

  return server;
}
