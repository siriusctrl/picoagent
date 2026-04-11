import { afterEach, expect, test } from 'bun:test';
import type { LocalServerHandle } from '../../src/http/bun-server.ts';
import { startHttpServer } from '../../src/http/server.ts';
import { startSessionServer } from '../../src/http/session-server.ts';
import { HttpSessionStore } from '../../src/runtime/http-session-store.ts';
import { makeTempDir, removeDir } from '../helpers/fs.ts';

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

function serverBaseUrl(server: LocalServerHandle): string {
  return server.url.origin;
}

test('session server returns 400 for malformed JSON on optional-body routes', async () => {
  const sessionRoot = await makeTempDir('picoagent-session-server-');
  runtimeRoots.add(sessionRoot);

  const sessionServer = await startSessionServer({
    cwd: process.cwd(),
    hostname: '127.0.0.1',
    port: 0,
    runtimeRoot: sessionRoot,
  });
  servers.add(sessionServer);
  const baseUrl = serverBaseUrl(sessionServer);

  const createResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"agent":',
  });
  expect(createResponse.status).toBe(400);
  expect(await createResponse.json()).toEqual({ error: 'Malformed JSON in request body' });

  const validCreateResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  expect(validCreateResponse.status).toBe(201);
  const session = (await validCreateResponse.json()) as { id: string };

  const compactResponse = await fetch(`${baseUrl}/sessions/${session.id}/compact`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"keepLastMessages":',
  });
  expect(compactResponse.status).toBe(400);
  expect(await compactResponse.json()).toEqual({ error: 'Malformed JSON in request body' });
});

test('session server returns 400 for malformed JSON on required-body store routes', async () => {
  const sessionRoot = await makeTempDir('picoagent-session-server-');
  runtimeRoots.add(sessionRoot);

  const sessionServer = await startSessionServer({
    cwd: process.cwd(),
    hostname: '127.0.0.1',
    port: 0,
    runtimeRoot: sessionRoot,
  });
  servers.add(sessionServer);
  const baseUrl = serverBaseUrl(sessionServer);

  const createResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  expect(createResponse.status).toBe(201);

  const storeResponse = await fetch(`${baseUrl}/_store/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"id":',
  });
  expect(storeResponse.status).toBe(400);
  expect(await storeResponse.json()).toEqual({ error: 'Malformed JSON in request body' });
});

async function waitForRun(baseUrl: string, runId: string): Promise<void> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const response = await fetch(`${baseUrl}/runs/${runId}`);
    expect(response.status).toBe(200);
    const payload = (await response.json()) as { status: string };
    if (payload.status === 'completed' || payload.status === 'failed') {
      return;
    }

    await new Promise((resolve) => setTimeout(resolve, 20));
  }

  throw new Error(`Run ${runId} did not finish in time`);
}

test('runtime creates sessions through the bound external session service', async () => {
  const sessionRoot = await makeTempDir('picoagent-session-server-');
  const runtimeRoot = await makeTempDir('picoagent-runtime-server-');
  runtimeRoots.add(sessionRoot);
  runtimeRoots.add(runtimeRoot);

  const sessionServer = await startSessionServer({
    cwd: process.cwd(),
    hostname: '127.0.0.1',
    port: 0,
    runtimeRoot: sessionRoot,
  });
  servers.add(sessionServer);
  const sessionBaseUrl = serverBaseUrl(sessionServer);

  const runtimeServer = await startHttpServer({
    cwd: process.cwd(),
    hostname: '127.0.0.1',
    port: 0,
    runtimeRoot,
    sessionStore: new HttpSessionStore(sessionBaseUrl),
  });
  servers.add(runtimeServer);
  const runtimeBaseUrl = serverBaseUrl(runtimeServer);

  const sessionCreateResponse = await fetch(`${runtimeBaseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  expect(sessionCreateResponse.status).toBe(201);
  const createdSession = (await sessionCreateResponse.json()) as { id: string };

  const runResponse = await fetch(`${runtimeBaseUrl}/sessions/${createdSession.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello external session' }),
  });
  expect(runResponse.status).toBe(202);
  const createdRun = (await runResponse.json()) as { runId: string };

  await waitForRun(runtimeBaseUrl, createdRun.runId);

  const sessionSnapshotResponse = await fetch(`${sessionBaseUrl}/sessions/${createdSession.id}`);
  expect(sessionSnapshotResponse.status).toBe(200);
  const sessionSnapshot = (await sessionSnapshotResponse.json()) as {
    id: string;
    activeRunId?: string;
    runs: Array<{ id: string }>;
  };

  expect(sessionSnapshot.id).toBe(createdSession.id);
  expect(sessionSnapshot.activeRunId).toBeUndefined();
  expect(sessionSnapshot.runs.map((run) => run.id)).toEqual([createdRun.runId]);
});
