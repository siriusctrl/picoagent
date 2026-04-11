import { $, OpenAPIHono } from '@hono/zod-openapi';
import type { ExecutionBackend } from '../core/execution.ts';
import type { MutableFilesystem } from '../core/filesystem.ts';
import type { NamespaceMount } from '../fs/namespace.ts';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.ts';
import { LocalExecutionBackend } from '../runtime/local-execution-backend.ts';
import type { RunEvent } from '../runtime/store.ts';
import type { SessionStore } from '../runtime/store.ts';
import { RuntimeService } from '../runtime/service.ts';
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
} from './openapi.ts';
import { type LocalServerHandle, startBunFetchServer } from './bun-server.ts';
import { projectSessionSummary } from './session-summary.ts';

export interface HttpServerOptions {
  cwd?: string;
  hostname?: string;
  port?: number;
  filesystem?: MutableFilesystem;
  mounts?: NamespaceMount[];
  executionBackend?: ExecutionBackend;
  sessionStore?: SessionStore;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

export interface HttpAppOptions {
  cwd?: string;
  filesystem?: MutableFilesystem;
  mounts?: NamespaceMount[];
  executionBackend?: ExecutionBackend;
  sessionStore?: SessionStore;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
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

export async function createHttpApp(options: HttpAppOptions = {}) {
  const service = await RuntimeService.create({
    cwd: options.cwd,
    filesystem: options.filesystem ?? new LocalWorkspaceFileSystem(),
    mounts: options.mounts,
    executionBackend: options.executionBackend ?? new LocalExecutionBackend(),
    sessionStore: options.sessionStore,
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
    const run = await service.createStandaloneRun(body.prompt);
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
    c.req.valid('json');
    const session = await service.createSession();
    return c.json(projectSessionSummary(session), 201);
  });

  const appWithGetSession = appWithCreateSession.openapi(getSessionRoute, async (c) => {
    const { sessionId } = c.req.valid('param');
    return c.json(await service.getSessionSnapshot(sessionId), 200);
  });

  const appWithCreateSessionRun = appWithGetSession.openapi(createSessionRunRoute, async (c) => {
    const { sessionId } = c.req.valid('param');
    const body = c.req.valid('json');
    const run = await service.createSessionRun(sessionId, body.prompt);
    return c.json({ runId: run.id, status: run.status, sessionId: run.sessionId }, 202);
  });

  const appWithListResources = appWithCreateSessionRun.openapi(listSessionResourcesRoute, async (c) => {
    const { sessionId } = c.req.valid('param');
    const query = c.req.valid('query');
    const resourcePath = query.path ?? '.';
    return c.json(
      {
        sessionId,
        path: resourcePath,
        entries: await service.listSessionResources(sessionId, resourcePath),
      },
      200,
    );
  });

  const appWithReadResource = appWithListResources.openapi(readSessionResourceRoute, async (c) => {
    const { sessionId, resourcePath } = c.req.valid('param');
    return c.text(await service.readSessionResource(sessionId, resourcePath), 200, {
      'content-type': resourcePath.endsWith('.jsonl')
        ? 'application/x-ndjson; charset=utf-8'
        : 'text/plain; charset=utf-8',
    });
  });

  const finalApp = appWithReadResource.openapi(compactSessionRoute, async (c) => {
    const { sessionId } = c.req.valid('param');
    const body = c.req.valid('json');
    return c.json(await service.compactSession(sessionId, body.keepLastMessages), 200);
  });

  return {
    app: finalApp,
    service,
  };
}

export type HttpAppType = Awaited<ReturnType<typeof createHttpApp>>['app'];

export async function startHttpServer(options: HttpServerOptions = {}): Promise<LocalServerHandle> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4096;
  const { app } = await createHttpApp(options);
  return startBunFetchServer(app.fetch, hostname, port);
}
