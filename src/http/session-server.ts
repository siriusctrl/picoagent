import http from 'node:http';
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
import { startNodeFetchServer } from './node-server.js';
import { projectSessionSummary } from './session-summary.js';

export interface SessionServerOptions extends SessionServiceOptions {
  hostname?: string;
  port?: number;
}

async function parseJsonRequest(request: Request, optional = false): Promise<unknown> {
  const body = await request.text();
  if (body.trim() === '') {
    if (optional) {
      return {};
    }

    throw new SessionValidationError('Request body is required');
  }

  try {
    return JSON.parse(body) as unknown;
  } catch {
    throw new SessionValidationError('Malformed JSON in request body');
  }
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
    const body = (await parseJsonRequest(c.req.raw, true)) as { agent?: unknown };
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
    const body = (await parseJsonRequest(c.req.raw)) as { agent?: unknown };
    const agent = parseAgent(body.agent);
    if (!agent) {
      throw new SessionValidationError('agent is required');
    }
    return c.json(await service.setSessionAgent(c.req.param('sessionId'), agent), 200);
  });

  app.post('/sessions/:sessionId/compact', async (c) => {
    const body = (await parseJsonRequest(c.req.raw, true)) as { keepLastMessages?: unknown };
    const keepLastMessages = body.keepLastMessages === undefined ? undefined : Number(body.keepLastMessages);
    return c.json(await service.compactSession(c.req.param('sessionId'), keepLastMessages), 200);
  });

  app.get('/_store/sessions/:sessionId', async (c) => {
    return c.json(await service.getSession(c.req.param('sessionId')), 200);
  });

  app.post('/_store/sessions', async (c) => {
    const record = (await parseJsonRequest(c.req.raw)) as SessionRecord;
    return c.json(await service.createSessionRecord(record), 201);
  });

  app.post('/_store/runs', async (c) => {
    await service.createRunRecord((await parseJsonRequest(c.req.raw)) as RunRecord);
    return c.json({ ok: true }, 201);
  });

  app.post('/_store/runs/:runId', async (c) => {
    await service.updateRunRecord(
      c.req.param('runId'),
      (await parseJsonRequest(c.req.raw)) as Partial<Omit<RunRecord, 'id' | 'events'>>,
    );
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/runs/:runId/events', async (c) => {
    await service.appendRunEvent(
      c.req.param('runId'),
      await parseJsonRequest(c.req.raw) as Parameters<SessionService['appendRunEvent']>[1],
    );
    return c.json({ ok: true }, 201);
  });

  app.post('/_store/sessions/:sessionId/control', async (c) => {
    await service.refreshSessionControl(c.req.param('sessionId'), await parseJsonRequest(c.req.raw) as {
      controlVersion: SessionRecord['controlVersion'];
      controlConfig: SessionRecord['controlConfig'];
      systemPrompts: SessionRecord['systemPrompts'];
    });
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/sessions/:sessionId/attach-run', async (c) => {
    const body = (await parseJsonRequest(c.req.raw)) as { runId: string };
    await service.attachRunToSession(c.req.param('sessionId'), body.runId);
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/sessions/:sessionId/finish-run', async (c) => {
    const body = (await parseJsonRequest(c.req.raw)) as { runId: string; messages: SessionRecord['messages'] };
    await service.finishSessionRun(c.req.param('sessionId'), body.runId, body.messages);
    return c.json({ ok: true }, 200);
  });

  app.post('/_store/sessions/:sessionId/clear-active-run', async (c) => {
    const body = (await parseJsonRequest(c.req.raw)) as { runId: string };
    await service.clearSessionActiveRun(c.req.param('sessionId'), body.runId);
    return c.json({ ok: true }, 200);
  });

  return { app, service };
}

export async function startSessionServer(options: SessionServerOptions = {}): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4097;
  const { app } = createSessionApp(options);
  return startNodeFetchServer(app.fetch, hostname, port);
}
