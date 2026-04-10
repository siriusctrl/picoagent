import { afterEach, expect, test } from 'bun:test';
import { joinPath } from '../../src/fs/path.ts';
import {
  createHttpApp,
  startHttpServer,
  type HttpAppType,
} from '../../src/http/server.ts';
import type { LocalServerHandle } from '../../src/http/bun-server.ts';
import { ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

type RunEvent = { type: string; [key: string]: unknown };
type RunStatus = 'running' | 'completed' | 'failed';
type HttpClient = {
  request(path: string, init?: RequestInit): Promise<Response>;
};

const servers = new Set<LocalServerHandle>();
const runtimeRoots = new Set<string>();

afterEach(async () => {
  await Promise.all(Array.from(servers, (server) => server.stop(true)));
  servers.clear();

  for (const runtimeRoot of runtimeRoots) {
    await removeDir(runtimeRoot);
  }
  runtimeRoots.clear();
});

async function stopServer(server: LocalServerHandle): Promise<void> {
  await server.stop(true);
  servers.delete(server);
}

async function startServer(
  cwd = process.cwd(),
  runtimeRoot?: string,
): Promise<{ baseUrl: string; client: HttpClient; server: LocalServerHandle; runtimeRoot: string }> {
  const resolvedRuntimeRoot = runtimeRoot ?? await makeTempDir('picoagent-http-runtime-');
  runtimeRoots.add(resolvedRuntimeRoot);
  const server = await startHttpServer({
    cwd,
    hostname: '127.0.0.1',
    port: 0,
    runtimeRoot: resolvedRuntimeRoot,
  });
  servers.add(server);

  return {
    baseUrl: server.url.origin,
    client: {
      request: (path, init) => fetch(`${server.url.origin}${path}`, init),
    },
    server,
    runtimeRoot: resolvedRuntimeRoot,
  };
}

async function startApp(
  cwd = process.cwd(),
  runtimeRoot?: string,
): Promise<{ client: HttpClient; runtimeRoot: string }> {
  const resolvedRuntimeRoot = runtimeRoot ?? await makeTempDir('picoagent-http-runtime-');
  runtimeRoots.add(resolvedRuntimeRoot);
  const { app }: { app: HttpAppType } = await createHttpApp({
    cwd,
    runtimeRoot: resolvedRuntimeRoot,
  });

  return {
    client: {
      request: async (path, init) => {
        return app.request(new URL(path, 'http://127.0.0.1'), init) as Promise<Response>;
      },
    },
    runtimeRoot: resolvedRuntimeRoot,
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

async function waitForRun(client: HttpClient, runId: string): Promise<{
  id: string;
  sessionId?: string;
  agent: string;
  status: RunStatus;
  prompt: string;
  output: string;
  error?: string;
}> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const response = await client.request(`/runs/${runId}`);
    expect(response.status).toBe(200);
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
  const { client } = await startApp();

  const createResponse = await client.request('/runs', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello' }),
  });

  expect(createResponse.status).toBe(202);
  const created = (await createResponse.json()) as { runId: string; status: RunStatus };
  expect(created.runId).toMatch(/^[0-9a-f-]{36}$/);
  expect(created.status).toBe('running');

  const run = await waitForRun(client, created.runId);
  expect(run.id).toBe(created.runId);
  expect(run.status).toBe('completed');
  expect(run.agent).toBe('ask');
  expect(run.prompt).toBe('hello');
  expect(run.output).toBe('received: hello');
});

test('GET /events/:runId returns the full event log as JSON', async () => {
  const { client } = await startApp();

  const createResponse = await client.request('/runs', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello json events' }),
  });
  const created = (await createResponse.json()) as { runId: string };

  await waitForRun(client, created.runId);

  const response = await client.request(`/events/${created.runId}`);
  expect(response.status).toBe(200);

  const payload = (await response.json()) as {
    runId: string;
    status: RunStatus;
    events: RunEvent[];
  };

  expect(payload.runId).toBe(created.runId);
  expect(payload.status).toBe('completed');
  expect(payload.events[0]?.type).toBe('run_started');
  expect(payload.events.at(-1)?.type).toBe('done');

  const deltaEvents = payload.events.filter((event) => event.type === 'assistant_delta');
  expect(deltaEvents).not.toHaveLength(0);
  expect(
    deltaEvents.map((event) => String(event.text ?? '')).join(''),
    'received: hello json events',
  ).toBe('received: hello json events');
});

test('GET /events/:runId streams the same run events over SSE', async () => {
  const { client } = await startServer();

  const createResponse = await client.request('/runs', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello stream events' }),
  });
  const created = (await createResponse.json()) as { runId: string };

  const response = await client.request(`/events/${created.runId}`, {
    headers: { accept: 'text/event-stream' },
  });
  expect(response.status).toBe(200);
  expect(response.headers.get('content-type') ?? '').toMatch(/^text\/event-stream\b/);

  const events = await readEvents(response, (all) => all.some((event) => event.type === 'done'));
  expect(events[0]?.type).toBe('run_started');
  expect(events.at(-1)?.type).toBe('done');

  const doneEvent = events.find((event) => event.type === 'done');
  expect(doneEvent?.output).toBe('received: hello stream events');
});

test('GET /events/:runId returns 404 for unknown SSE runs without killing the server', async () => {
  const { client } = await startServer();

  const response = await client.request('/events/missing-run-id', {
    headers: { accept: 'text/event-stream' },
  });
  expect(response.status).toBe(404);

  const payload = (await response.json()) as { error: string };
  expect(payload.error).toMatch(/Run missing-run-id not found/);

  const healthCheck = await client.request('/openapi');
  expect(healthCheck.status).toBe(200);
});

test('sessions keep ordered run history and expose the related run ids', async () => {
  const { client } = await startApp();

  const createSessionResponse = await client.request('/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  expect(createSessionResponse.status).toBe(201);

  const session = (await createSessionResponse.json()) as {
    id: string;
    agent: string;
    controlVersion: string;
    controlConfig: { provider: string };
  };
  expect(session.agent).toBe('ask');
  expect(session.controlConfig.provider).toBe('echo');

  const firstRunResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'first turn' }),
  });
  expect(firstRunResponse.status).toBe(202);
  const firstRun = (await firstRunResponse.json()) as { runId: string; sessionId: string };
  expect(firstRun.sessionId).toBe(session.id);

  await waitForRun(client, firstRun.runId);

  const secondRunResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'second turn' }),
  });
  expect(secondRunResponse.status).toBe(202);
  const secondRun = (await secondRunResponse.json()) as { runId: string; sessionId: string };

  await waitForRun(client, secondRun.runId);

  const sessionResponse = await client.request(`/sessions/${session.id}`);
  expect(sessionResponse.status).toBe(200);

  const snapshot = (await sessionResponse.json()) as {
    id: string;
    agent: string;
    controlVersion: string;
    controlConfig: { provider: string; model: string };
    runs: Array<{ id: string; agent: string; status: RunStatus; prompt: string; output: string }>;
  };

  expect(snapshot.id).toBe(session.id);
  expect(snapshot.agent).toBe('ask');
  expect(snapshot.controlVersion).toMatch(/^[0-9a-f]{64}$/);
  expect(snapshot.controlConfig.provider).toBe('echo');
  expect(snapshot.runs.map((run) => run.id)).toEqual([firstRun.runId, secondRun.runId]);
  expect(snapshot.runs.map((run) => run.prompt)).toEqual(['first turn', 'second turn']);
});

test('sessions keep a default agent and allow it to be updated over HTTP', async () => {
  const { client } = await startApp();

  const createSessionResponse = await client.request('/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const agentResponse = await client.request(`/sessions/${session.id}/agent`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'exec' }),
  });
  expect(agentResponse.status).toBe(200);

  const updated = (await agentResponse.json()) as { id: string; agent: string };
  expect(updated.id).toBe(session.id);
  expect(updated.agent).toBe('exec');

  const runResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'uses default agent' }),
  });
  const run = (await runResponse.json()) as { runId: string };
  const snapshot = await waitForRun(client, run.runId);
  expect(snapshot.agent).toBe('exec');
});

test('POST /sessions/:id/agent rejects missing agent values', async () => {
  const { client } = await startApp();

  const createSessionResponse = await client.request('/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'exec' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const agentResponse = await client.request(`/sessions/${session.id}/agent`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  expect(agentResponse.status).toBe(400);

  const errorPayload = (await agentResponse.json()) as { error: string };
  expect(errorPayload.error).toBe('agent is required');

  const sessionResponse = await client.request(`/sessions/${session.id}`);
  const snapshot = (await sessionResponse.json()) as { agent: string };
  expect(snapshot.agent).toBe('exec');
});

test('session runs inherit the session default agent unless the request overrides it', async () => {
  const { client } = await startApp();

  const createSessionResponse = await client.request('/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const runResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'override just this run', agent: 'exec' }),
  });
  expect(runResponse.status).toBe(202);

  const run = (await runResponse.json()) as { runId: string };
  const snapshot = await waitForRun(client, run.runId);
  expect(snapshot.agent).toBe('exec');

  const sessionResponse = await client.request(`/sessions/${session.id}`);
  const updatedSession = (await sessionResponse.json()) as { agent: string };
  expect(updatedSession.agent).toBe('ask');
});

test('GET /openapi documents the async run and event endpoints', async () => {
  const { client } = await startApp();

  const response = await client.request('/openapi');
  expect(response.status).toBe(200);

  const document = (await response.json()) as {
    paths: Record<string, Record<string, { description?: string; responses?: Record<string, { content?: Record<string, unknown> }> }>>;
  };

  expect(document.paths).toHaveProperty('/runs');
  expect(document.paths['/events/{runId}']?.get?.description ?? '').toContain('Accept: text/event-stream');
  expect(document.paths).toHaveProperty('/sessions/{sessionId}/runs');
  expect(document.paths).toHaveProperty('/sessions/{sessionId}/resources');
  expect(document.paths).toHaveProperty('/sessions/{sessionId}/resources/{resourcePath}');
  expect(document.paths).toHaveProperty('/sessions/{sessionId}/agent');
  expect(document.paths).toHaveProperty('/sessions/{sessionId}/compact');
});

test('POST endpoints return 400 for malformed JSON bodies', async () => {
  const { client } = await startApp();

  const response = await client.request('/runs', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"prompt":',
  });
  expect(response.status).toBe(400);

  const payload = (await response.json()) as { error: string };
  expect(payload.error).toBe('Malformed JSON in request body');
});

test('session runs automatically refresh control inputs when the workspace changes', async () => {
  const root = await makeTempDir('picoagent-http-workspace-');

  try {
    await ensureDir(joinPath(root, '.pico'));
    await writeTextFile(joinPath(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }\n');

    const { client } = await startApp(root);

    const createSessionResponse = await client.request('/sessions', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ agent: 'ask' }),
    });
    expect(createSessionResponse.status).toBe(201);

    const session = (await createSessionResponse.json()) as {
      id: string;
      controlVersion: string;
      controlConfig: { provider: string };
    };
    expect(session.controlConfig.provider).toBe('echo');

    await writeTextFile(joinPath(root, '.pico', 'config.jsonc'), '{ "provider": "wat", "model": "echo" }\n');

    const runResponse = await client.request(`/sessions/${session.id}/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ prompt: 'should fail after refresh' }),
    });
    expect(runResponse.status).toBe(500);

    const errorPayload = (await runResponse.json()) as { error: string };
    expect(errorPayload.error).toMatch(/invalid provider \"wat\"/);

    const sessionResponse = await client.request(`/sessions/${session.id}`);
    expect(sessionResponse.status).toBe(200);
    const snapshot = (await sessionResponse.json()) as {
      controlVersion: string;
      controlConfig: { provider: string };
    };
    expect(snapshot.controlVersion).toBe(session.controlVersion);
    expect(snapshot.controlConfig.provider).toBe('echo');
  } finally {
    await removeDir(root);
  }
});

test('sessions compact into checkpoints and expose the compacted snapshot over HTTP', async () => {
  const { client } = await startApp();

  const createSessionResponse = await client.request('/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  expect(createSessionResponse.status).toBe(201);

  const session = (await createSessionResponse.json()) as { id: string };

  const firstRunResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'first turn to compact' }),
  });
  expect(firstRunResponse.status).toBe(202);
  const firstRun = (await firstRunResponse.json()) as { runId: string };
  await waitForRun(client, firstRun.runId);

  const secondRunResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'second turn to keep' }),
  });
  expect(secondRunResponse.status).toBe(202);
  const secondRun = (await secondRunResponse.json()) as { runId: string };
  await waitForRun(client, secondRun.runId);

  const compactResponse = await client.request(`/sessions/${session.id}/compact`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ keepLastMessages: 2 }),
  });
  expect(compactResponse.status).toBe(200);

  const compacted = (await compactResponse.json()) as {
    checkpoint: { checkpointId: string; compactedMessages: number; keptMessages: number; summary: string };
    session: { id: string; activeCheckpointId?: string; checkpointCount: number };
  };
  expect(compacted.session.id).toBe(session.id);
  expect(compacted.session.activeCheckpointId).toBe(compacted.checkpoint.checkpointId);
  expect(compacted.session.checkpointCount).toBe(1);
  expect(compacted.checkpoint.compactedMessages).toBe(2);
  expect(compacted.checkpoint.keptMessages).toBe(2);
  expect(compacted.checkpoint.summary).toMatch(/first turn to compact/);

  const sessionResponse = await client.request(`/sessions/${session.id}`);
  expect(sessionResponse.status).toBe(200);
  const snapshot = (await sessionResponse.json()) as {
    activeCheckpointId?: string;
    checkpointCount: number;
  };
  expect(snapshot.activeCheckpointId).toBe(compacted.checkpoint.checkpointId);
  expect(snapshot.checkpointCount).toBe(1);
});

test('session history resources are available over HTTP', async () => {
  const { client } = await startApp();

  const createSessionResponse = await client.request('/sessions', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'exec' }),
  });
  const session = (await createSessionResponse.json()) as { id: string };

  const runResponse = await client.request(`/sessions/${session.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'resource test prompt' }),
  });
  const run = (await runResponse.json()) as { runId: string };
  await waitForRun(client, run.runId);

  await client.request(`/sessions/${session.id}/compact`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ keepLastMessages: 1 }),
  });

  const listResponse = await client.request(`/sessions/${session.id}/resources`);
  expect(listResponse.status).toBe(200);
  const listing = (await listResponse.json()) as { entries: string[] };
  expect(listing.entries).toEqual(['summary.md', 'checkpoints/', 'runs/', 'events/']);

  const runsListResponse = await client.request(`/sessions/${session.id}/resources?path=runs`);
  expect(runsListResponse.status).toBe(200);
  const runsListing = (await runsListResponse.json()) as { entries: string[] };
  expect(runsListing.entries).toEqual([`${run.runId}.md`]);

  const summaryResponse = await client.request(`/sessions/${session.id}/resources/summary.md`);
  expect(summaryResponse.status).toBe(200);
  expect(summaryResponse.headers.get('content-type') ?? '').toMatch(/^text\/plain\b/);
  expect(await summaryResponse.text()).toMatch(/# Checkpoint/);

  const eventsResponse = await client.request(`/sessions/${session.id}/resources/events/${run.runId}.jsonl`);
  expect(eventsResponse.status).toBe(200);
  expect(eventsResponse.headers.get('content-type') ?? '').toMatch(/^application\/x-ndjson\b/);
  expect(await eventsResponse.text()).toMatch(/"type":"run_started"/);
});

test('sessions and runs survive a server restart through the file runtime store', async () => {
  const root = await makeTempDir('picoagent-http-persist-workspace-');
  const runtimeRoot = await makeTempDir('picoagent-http-persist-runtime-');
  runtimeRoots.add(root);

  try {
    const first = await startServer(root, runtimeRoot);
    const { client: firstClient } = first;

    const createSessionResponse = await firstClient.request('/sessions', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ agent: 'ask' }),
    });
    const session = (await createSessionResponse.json()) as { id: string };

    const runResponse = await firstClient.request(`/sessions/${session.id}/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ prompt: 'persist across restart' }),
    });
    const run = (await runResponse.json()) as { runId: string };
    await waitForRun(firstClient, run.runId);

    const compactResponse = await firstClient.request(`/sessions/${session.id}/compact`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ keepLastMessages: 1 }),
    });
    expect(compactResponse.status).toBe(200);

    await stopServer(first.server);

    const second = await startServer(root, runtimeRoot);
    const { client: secondClient } = second;

    const sessionResponse = await secondClient.request(`/sessions/${session.id}`);
    expect(sessionResponse.status).toBe(200);
    const snapshot = (await sessionResponse.json()) as { checkpointCount: number; runs: Array<{ id: string }> };
    expect(snapshot.checkpointCount).toBe(1);
    expect(snapshot.runs.map((item) => item.id)).toEqual([run.runId]);

    const runSnapshotResponse = await secondClient.request(`/runs/${run.runId}`);
    expect(runSnapshotResponse.status).toBe(200);
    const runSnapshot = (await runSnapshotResponse.json()) as { status: RunStatus; output: string };
    expect(runSnapshot.status).toBe('completed');
    expect(runSnapshot.output).toBe('received: persist across restart');

    const eventsResponse = await secondClient.request(`/sessions/${session.id}/resources/events/${run.runId}.jsonl`);
    expect(eventsResponse.status).toBe(200);
    expect(await eventsResponse.text()).toMatch(/"type":"done"/);
  } finally {
    await removeDir(root);
    runtimeRoots.delete(root);
  }
});
