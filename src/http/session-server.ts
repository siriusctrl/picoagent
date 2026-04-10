import http from 'node:http';
import { createAdaptorServer } from '@hono/node-server';
import { Hono } from 'hono';
import { AgentPresetId } from '../core/types.js';
import {
  SessionConflictError,
  SessionNotFoundError,
  SessionService,
  SessionValidationError,
  type SessionServiceOptions,
} from '../runtime/session-service.js';
import type { RunRecord, SessionRecord } from '../runtime/store.js';

export interface SessionServerOptions extends SessionServiceOptions {
  hostname?: string;
  port?: number;
}

function projectSessionSummary(session: SessionRecord) {
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
  if (
    error instanceof SessionNotFoundError
    || error instanceof SessionConflictError
    || error instanceof SessionValidationError
  ) {
    return error.status;
  }

  return 500;
}

function parseAgent(value: unknown): AgentPresetId | undefined {
  if (value === undefined) {
    return undefined;
  }

  if (value === 'ask' || value === 'exec') {
    return value;
  }

  throw new SessionValidationError(`Unsupported agent: ${String(value)}`);
}

export function createSessionApp(options: SessionServerOptions = {}) {
  const service = new SessionService(options);
  const app = new Hono();

  app.onError((error, c) => {
    return c.json({ error: error instanceof Error ? error.message : String(error) }, errorStatus(error));
  });

  app.post('/sessions', async (c) => {
    const body = (await c.req.json().catch(() => ({}))) as { agent?: unknown };
    const session = await service.createSession(parseAgent(body.agent) ?? 'ask');
    return c.json(projectSessionSummary(session), 201);
  });

  app.get('/sessions/:sessionId', async (c) => {
    return c.json(await service.getSessionSnapshot(c.req.param('sessionId')), 200);
  });

  app.get('/sessions/:sessionId/resources', async (c) => {
    const sessionId = c.req.param('sessionId');
    const resourcePath = c.req.query('path') ?? '.';
    return c.json({
      sessionId,
      path: resourcePath,
      entries: await service.listSessionResources(sessionId, resourcePath),
    }, 200);
  });

  app.get('/sessions/:sessionId/resources/:resourcePath{.+}', async (c) => {
    const sessionId = c.req.param('sessionId');
    const resourcePath = c.req.param('resourcePath');
    const content = await service.readSessionResource(sessionId, resourcePath);
    return c.text(content, 200, {
      'content-type': resourcePath.endsWith('.jsonl')
        ? 'application/x-ndjson; charset=utf-8'
        : 'text/plain; charset=utf-8',
    });
  });

  app.post('/sessions/:sessionId/agent', async (c) => {
    const body = (await c.req.json()) as { agent?: unknown };
    const agent = parseAgent(body.agent);
    if (!agent) {
      throw new SessionValidationError('agent is required');
    }
    return c.json(await service.setSessionAgent(c.req.param('sessionId'), agent), 200);
  });

  app.post('/sessions/:sessionId/compact', async (c) => {
    const body = (await c.req.json().catch(() => ({}))) as { keepLastMessages?: unknown };
    const keepLastMessages = body.keepLastMessages === undefined ? undefined : Number(body.keepLastMessages);
    return c.json(await service.compactSession(c.req.param('sessionId'), keepLastMessages), 200);
  });

  app.get('/_store/sessions/:sessionId', async (c) => {
    return c.json(await service.getSession(c.req.param('sessionId')), 200);
  });

  app.post('/_store/sessions', async (c) => {
    const record = (await c.req.json()) as SessionRecord;
    return c.json(await service.createSessionRecord(record), 201);
  });

  app.post('/_store/runs', async (c) => {
    await service.createRunRecord((await c.req.json()) as RunRecord);
    return c.json({ ok: true }, 201);
  });

  app.post('/_store/runs/:runId', async (c) => {
    await service.updateRunRecord(c.req.param('runId'), await c.req.json());
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/runs/:runId/events', async (c) => {
    await service.appendRunEvent(c.req.param('runId'), await c.req.json());
    return c.json({ ok: true }, 201);
  });

  app.post('/_store/sessions/:sessionId/control', async (c) => {
    await service.refreshSessionControl(c.req.param('sessionId'), await c.req.json());
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/sessions/:sessionId/attach-run', async (c) => {
    const body = (await c.req.json()) as { runId: string };
    await service.attachRunToSession(c.req.param('sessionId'), body.runId);
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/sessions/:sessionId/finish-run', async (c) => {
    const body = (await c.req.json()) as { runId: string; messages: SessionRecord['messages'] };
    await service.finishSessionRun(c.req.param('sessionId'), body.runId, body.messages);
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/sessions/:sessionId/clear-active-run', async (c) => {
    const body = (await c.req.json()) as { runId: string };
    await service.clearSessionActiveRun(c.req.param('sessionId'), body.runId);
    return c.json({ ok: true }, 200);
  });

  return { app, service };
}

export async function startSessionServer(options: SessionServerOptions = {}): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4097;
  const { app } = createSessionApp(options);

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
