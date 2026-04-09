import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';
import type http from 'node:http';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { startHttpServer } from '../../src/http/server.js';

type RunEvent = { type: string; [key: string]: unknown };
type RunStatus = 'running' | 'completed' | 'failed';

const servers = new Set<http.Server>();

afterEach(async () => {
  await Promise.all(
    Array.from(servers, (server) => new Promise<void>((resolve, reject) => {
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolve();
      });
    })),
  );
  servers.clear();
});

async function startServer(cwd = process.cwd()): Promise<{ baseUrl: string }> {
  const server = await startHttpServer({
    cwd,
    hostname: '127.0.0.1',
    port: 0,
  });
  servers.add(server);

  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Expected an inet server address');
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
  };
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

async function readEvents(response: Response, until: (events: RunEvent[]) => boolean): Promise<RunEvent[]> {
  const reader = response.body?.getReader();
  if (!reader) {
    throw new Error('Expected response body');
  }

  const decoder = new TextDecoder();
  const events: RunEvent[] = [];
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }

    buffer += decoder.decode(value, { stream: true });
    let boundary = buffer.indexOf('\n\n');
    while (boundary >= 0) {
      const frame = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      const event = parseSseFrame(frame);
      if (event) {
        events.push(event);
        if (until(events)) {
          return events;
        }
      }

      boundary = buffer.indexOf('\n\n');
    }
  }

  return events;
}

async function waitForRun(baseUrl: string, runId: string): Promise<{
  id: string;
  sessionId?: string;
  agent: string;
  status: RunStatus;
  prompt: string;
  output: string;
  error?: string;
}> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const response = await fetch(`${baseUrl}/runs/${runId}`);
    assert.equal(response.status, 200);
    const payload = (await response.json()) as {
      id: string;
      sessionId?: string;
      agent: string;
      status: RunStatus;
      prompt: string;
      output: string;
      error?: string;
    };

    if (payload.status === 'completed' || payload.status === 'failed') {
      return payload;
    }

    await new Promise((resolve) => setTimeout(resolve, 20));
  }

  throw new Error(`Run ${runId} did not finish in time`);
}

test('POST /runs creates an async run and GET /runs/:id returns the final snapshot', async () => {
  const { baseUrl } = await startServer();

  const createResponse = await fetch(`${baseUrl}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello' }),
  });

  assert.equal(createResponse.status, 202);
  const created = (await createResponse.json()) as { runId: string; status: RunStatus };
  assert.match(created.runId, /^[0-9a-f-]{36}$/);
  assert.equal(created.status, 'running');

  const run = await waitForRun(baseUrl, created.runId);
  assert.equal(run.id, created.runId);
  assert.equal(run.status, 'completed');
  assert.equal(run.agent, 'ask');
  assert.equal(run.prompt, 'hello');
  assert.equal(run.output, 'received: hello');
});

test('GET /events/:runId returns the full event log as JSON', async () => {
  const { baseUrl } = await startServer();

  const createResponse = await fetch(`${baseUrl}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello json events' }),
  });
  const created = (await createResponse.json()) as { runId: string };

  await waitForRun(baseUrl, created.runId);

  const response = await fetch(`${baseUrl}/events/${created.runId}`);
  assert.equal(response.status, 200);

  const payload = (await response.json()) as {
    runId: string;
    status: RunStatus;
    events: RunEvent[];
  };

  assert.equal(payload.runId, created.runId);
  assert.equal(payload.status, 'completed');
  assert.equal(payload.events[0]?.type, 'run_started');
  assert.equal(payload.events.at(-1)?.type, 'done');

  const deltaEvents = payload.events.filter((event) => event.type === 'assistant_delta');
  assert.ok(deltaEvents.length >= 1);
  assert.equal(
    deltaEvents.map((event) => String(event.text ?? '')).join(''),
    'received: hello json events',
  );
});

test('GET /events/:runId streams the same run events over SSE', async () => {
  const { baseUrl } = await startServer();

  const createResponse = await fetch(`${baseUrl}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello stream events' }),
  });
  const created = (await createResponse.json()) as { runId: string };

  const response = await fetch(`${baseUrl}/events/${created.runId}`, {
    headers: { accept: 'text/event-stream' },
  });
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-type') ?? '', /^text\/event-stream\b/);

  const events = await readEvents(response, (all) => all.some((event) => event.type === 'done'));
  assert.equal(events[0]?.type, 'run_started');
  assert.equal(events.at(-1)?.type, 'done');

  const doneEvent = events.find((event) => event.type === 'done');
  assert.equal(doneEvent?.output, 'received: hello stream events');
});

test('GET /events/:runId returns 404 for unknown SSE runs without killing the server', async () => {
  const { baseUrl } = await startServer();

  const response = await fetch(`${baseUrl}/events/missing-run-id`, {
    headers: { accept: 'text/event-stream' },
  });
  assert.equal(response.status, 404);

  const payload = (await response.json()) as { error: string };
  assert.match(payload.error, /Run missing-run-id not found/);

  const healthCheck = await fetch(`${baseUrl}/openapi.json`);
  assert.equal(healthCheck.status, 200);
});

test('sessions keep ordered run history and expose the related run ids', async () => {
  const { baseUrl } = await startServer();

  const createSessionResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  assert.equal(createSessionResponse.status, 201);

  const session = (await createSessionResponse.json()) as {
    id: string;
    agent: string;
    controlVersion: string;
    controlConfig: { provider: string };
  };
  assert.equal(session.agent, 'ask');
  assert.equal(session.controlConfig.provider, 'echo');

  const firstRunResponse = await fetch(`${baseUrl}/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'first turn' }),
  });
  assert.equal(firstRunResponse.status, 202);
  const firstRun = (await firstRunResponse.json()) as { runId: string; sessionId: string };
  assert.equal(firstRun.sessionId, session.id);

  await waitForRun(baseUrl, firstRun.runId);

  const secondRunResponse = await fetch(`${baseUrl}/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'second turn' }),
  });
  assert.equal(secondRunResponse.status, 202);
  const secondRun = (await secondRunResponse.json()) as { runId: string; sessionId: string };

  await waitForRun(baseUrl, secondRun.runId);

  const sessionResponse = await fetch(`${baseUrl}/sessions/${session.id}`);
  assert.equal(sessionResponse.status, 200);

  const snapshot = (await sessionResponse.json()) as {
    id: string;
    agent: string;
    controlVersion: string;
    controlConfig: { provider: string; model: string };
    runs: Array<{ id: string; agent: string; status: RunStatus; prompt: string; output: string }>;
  };

  assert.equal(snapshot.id, session.id);
  assert.equal(snapshot.agent, 'ask');
  assert.match(snapshot.controlVersion, /^[0-9a-f]{64}$/);
  assert.equal(snapshot.controlConfig.provider, 'echo');
  assert.deepEqual(
    snapshot.runs.map((run) => run.id),
    [firstRun.runId, secondRun.runId],
  );
  assert.deepEqual(
    snapshot.runs.map((run) => run.prompt),
    ['first turn', 'second turn'],
  );
});

test('sessions keep a default agent and allow it to be updated over HTTP', async () => {
  const { baseUrl } = await startServer();

  const createSessionResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const agentResponse = await fetch(`${baseUrl}/sessions/${session.id}/agent`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'exec' }),
  });
  assert.equal(agentResponse.status, 200);

  const updated = (await agentResponse.json()) as { id: string; agent: string };
  assert.equal(updated.id, session.id);
  assert.equal(updated.agent, 'exec');

  const runResponse = await fetch(`${baseUrl}/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'uses default agent' }),
  });
  const run = (await runResponse.json()) as { runId: string };
  const snapshot = await waitForRun(baseUrl, run.runId);
  assert.equal(snapshot.agent, 'exec');
});

test('POST /sessions/:id/agent rejects missing agent values', async () => {
  const { baseUrl } = await startServer();

  const createSessionResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'exec' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const agentResponse = await fetch(`${baseUrl}/sessions/${session.id}/agent`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  assert.equal(agentResponse.status, 400);

  const errorPayload = (await agentResponse.json()) as { error: string };
  assert.equal(errorPayload.error, 'agent is required');

  const sessionResponse = await fetch(`${baseUrl}/sessions/${session.id}`);
  const snapshot = (await sessionResponse.json()) as { agent: string };
  assert.equal(snapshot.agent, 'exec');
});

test('session runs inherit the session default agent unless the request overrides it', async () => {
  const { baseUrl } = await startServer();

  const createSessionResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const runResponse = await fetch(`${baseUrl}/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'override just this run', agent: 'exec' }),
  });
  assert.equal(runResponse.status, 202);

  const run = (await runResponse.json()) as { runId: string };
  const snapshot = await waitForRun(baseUrl, run.runId);
  assert.equal(snapshot.agent, 'exec');

  const sessionResponse = await fetch(`${baseUrl}/sessions/${session.id}`);
  const updatedSession = (await sessionResponse.json()) as { agent: string };
  assert.equal(updatedSession.agent, 'ask');
});

test('GET /openapi.json documents the async run and event endpoints', async () => {
  const { baseUrl } = await startServer();

  const response = await fetch(`${baseUrl}/openapi.json`);
  assert.equal(response.status, 200);

  const document = (await response.json()) as {
    paths: Record<string, Record<string, { description?: string; responses?: Record<string, { content?: Record<string, unknown> }> }>>;
  };

  assert.ok(document.paths['/runs']);
  assert.ok(document.paths['/events/{runId}']?.get?.description?.includes('Accept: text/event-stream'));
  assert.ok(document.paths['/sessions/{sessionId}/runs']);
  assert.ok(document.paths['/sessions/{sessionId}/agent']);
});

test('POST endpoints return 400 for malformed JSON bodies', async () => {
  const { baseUrl } = await startServer();

  const response = await fetch(`${baseUrl}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"prompt":',
  });
  assert.equal(response.status, 400);

  const payload = (await response.json()) as { error: string };
  assert.match(payload.error, /^Invalid JSON body:/);
});

test('session runs automatically refresh control inputs when the workspace changes', async () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-http-workspace-'));

  try {
    mkdirSync(join(root, '.pico'), { recursive: true });
    writeFileSync(join(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }\n', 'utf8');

    const { baseUrl } = await startServer(root);

    const createSessionResponse = await fetch(`${baseUrl}/sessions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ agent: 'ask' }),
    });
    assert.equal(createSessionResponse.status, 201);

    const session = (await createSessionResponse.json()) as {
      id: string;
      controlVersion: string;
      controlConfig: { provider: string };
    };
    assert.equal(session.controlConfig.provider, 'echo');

    writeFileSync(join(root, '.pico', 'config.jsonc'), '{ "provider": "wat", "model": "echo" }\n', 'utf8');

    const runResponse = await fetch(`${baseUrl}/sessions/${session.id}/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ prompt: 'should fail after refresh' }),
    });
    assert.equal(runResponse.status, 500);

    const errorPayload = (await runResponse.json()) as { error: string };
    assert.match(errorPayload.error, /invalid provider "wat"/);

    const sessionResponse = await fetch(`${baseUrl}/sessions/${session.id}`);
    assert.equal(sessionResponse.status, 200);
    const snapshot = (await sessionResponse.json()) as {
      controlVersion: string;
      controlConfig: { provider: string };
    };
    assert.equal(snapshot.controlVersion, session.controlVersion);
    assert.equal(snapshot.controlConfig.provider, 'echo');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
