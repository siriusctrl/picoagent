import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';
import type http from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { startHttpServer } from '../../src/http/server.js';
import { startSessionServer } from '../../src/http/session-server.js';
import { HttpSessionStore } from '../../src/runtime/http-session-store.js';

const servers = new Set<http.Server>();
const runtimeRoots = new Set<string>();

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

  for (const runtimeRoot of runtimeRoots) {
    rmSync(runtimeRoot, { recursive: true, force: true });
  }
  runtimeRoots.clear();
});

function serverBaseUrl(server: http.Server): string {
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Expected an inet server address');
  }

  return `http://127.0.0.1:${address.port}`;
}

test('session server returns 400 for malformed JSON on optional-body routes', async () => {
  const sessionRoot = mkdtempSync(join(tmpdir(), 'picoagent-session-server-'));
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
  assert.equal(createResponse.status, 400);
  assert.deepEqual(await createResponse.json(), { error: 'Malformed JSON in request body' });

  const validCreateResponse = await fetch(`${baseUrl}/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: 'ask' }),
  });
  assert.equal(validCreateResponse.status, 201);
  const session = (await validCreateResponse.json()) as { id: string };

  const compactResponse = await fetch(`${baseUrl}/sessions/${session.id}/compact`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"keepLastMessages":',
  });
  assert.equal(compactResponse.status, 400);
  assert.deepEqual(await compactResponse.json(), { error: 'Malformed JSON in request body' });
});

test('session server returns 400 for malformed JSON on required-body routes', async () => {
  const sessionRoot = mkdtempSync(join(tmpdir(), 'picoagent-session-server-'));
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
    body: JSON.stringify({ agent: 'ask' }),
  });
  assert.equal(createResponse.status, 201);
  const session = (await createResponse.json()) as { id: string };

  const setAgentResponse = await fetch(`${baseUrl}/sessions/${session.id}/agent`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"agent":',
  });
  assert.equal(setAgentResponse.status, 400);
  assert.deepEqual(await setAgentResponse.json(), { error: 'Malformed JSON in request body' });

  const storeResponse = await fetch(`${baseUrl}/_store/sessions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{"id":',
  });
  assert.equal(storeResponse.status, 400);
  assert.deepEqual(await storeResponse.json(), { error: 'Malformed JSON in request body' });
});

async function waitForRun(baseUrl: string, runId: string): Promise<void> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const response = await fetch(`${baseUrl}/runs/${runId}`);
    assert.equal(response.status, 200);
    const payload = (await response.json()) as { status: string };
    if (payload.status === 'completed' || payload.status === 'failed') {
      return;
    }

    await new Promise((resolve) => setTimeout(resolve, 20));
  }

  throw new Error(`Run ${runId} did not finish in time`);
}

test('runtime creates sessions through the bound external session service', async () => {
  const sessionRoot = mkdtempSync(join(tmpdir(), 'picoagent-session-server-'));
  const runtimeRoot = mkdtempSync(join(tmpdir(), 'picoagent-runtime-server-'));
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
    body: JSON.stringify({ agent: 'ask' }),
  });
  assert.equal(sessionCreateResponse.status, 201);
  const createdSession = (await sessionCreateResponse.json()) as { id: string };

  const runResponse = await fetch(`${runtimeBaseUrl}/sessions/${createdSession.id}/runs`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ prompt: 'hello external session' }),
  });
  assert.equal(runResponse.status, 202);
  const createdRun = (await runResponse.json()) as { runId: string };

  await waitForRun(runtimeBaseUrl, createdRun.runId);

  const sessionSnapshotResponse = await fetch(`${sessionBaseUrl}/sessions/${createdSession.id}`);
  assert.equal(sessionSnapshotResponse.status, 200);
  const sessionSnapshot = (await sessionSnapshotResponse.json()) as {
    id: string;
    activeRunId?: string;
    runs: Array<{ id: string }>;
  };

  assert.equal(sessionSnapshot.id, createdSession.id);
  assert.equal(sessionSnapshot.activeRunId, undefined);
  assert.deepEqual(sessionSnapshot.runs.map((run) => run.id), [createdRun.runId]);
});
