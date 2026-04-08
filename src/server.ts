import { createServer, IncomingMessage, ServerResponse } from 'node:http';
import { existsSync, readFileSync } from 'fs';
import { join } from 'path';
import { createAppBootstrap } from './app/bootstrap.js';
import { listTasks, readTask } from './lib/task.js';

const port = parseInt(process.env.PICOAGENT_PORT || '3000', 10);
const app = createAppBootstrap(process.cwd(), {
  onBackgroundError: (error) => console.error(error),
});

function cors(res: ServerResponse): void {
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type');
}

function json(res: ServerResponse, data: unknown, status = 200): void {
  cors(res);
  res.writeHead(status, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify(data));
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let body = '';
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => resolve(body));
    req.on('error', reject);
  });
}

function parseRoute(url: string): { path: string; segments: string[] } {
  const path = url.split('?')[0];
  const segments = path.split('/').filter(Boolean);
  return { path, segments };
}

async function handleChat(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const raw = await readBody(req);
  let message: string;
  try {
    const body = JSON.parse(raw);
    message = body.message;
    if (typeof message !== 'string' || !message.trim()) {
      json(res, { error: 'missing "message" field' }, 400);
      return;
    }
  } catch {
    json(res, { error: 'invalid JSON body' }, 400);
    return;
  }

  cors(res);
  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-cache',
    'Connection': 'keep-alive',
  });

  try {
    const result = await app.runtime.onUserMessage(message, (delta) => {
      res.write(`data: ${JSON.stringify({ type: 'delta', text: delta })}\n\n`);
    });

    const text = result.content
      .filter((block) => block.type === 'text')
      .map((block) => block.text)
      .join('');

    res.write(`data: ${JSON.stringify({ type: 'done', text })}\n\n`);
  } catch (error: unknown) {
    const msg = error instanceof Error ? error.message : String(error);
    res.write(`data: ${JSON.stringify({ type: 'error', error: msg })}\n\n`);
  }

  res.end();
}

function handleListTasks(_req: IncomingMessage, res: ServerResponse): void {
  const tasks = listTasks(app.context.tasksRoot);
  json(res, { tasks });
}

function handleGetTask(taskId: string, _req: IncomingMessage, res: ServerResponse): void {
  const taskDir = join(app.context.tasksRoot, taskId);
  if (!existsSync(taskDir)) {
    json(res, { error: `task ${taskId} not found` }, 404);
    return;
  }

  const task = readTask(taskDir);
  const progressPath = join(taskDir, 'progress.md');
  const resultPath = join(taskDir, 'result.md');
  const progress = existsSync(progressPath) ? readFileSync(progressPath, 'utf-8') : null;
  const result = existsSync(resultPath) ? readFileSync(resultPath, 'utf-8') : null;

  json(res, { task, progress, result });
}

async function handleSteerTask(taskId: string, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const raw = await readBody(req);
  let message: string;
  try {
    const body = JSON.parse(raw);
    message = body.message;
    if (typeof message !== 'string') {
      json(res, { error: 'missing "message" field' }, 400);
      return;
    }
  } catch {
    json(res, { error: 'invalid JSON body' }, 400);
    return;
  }

  const control = app.runtime.getControl(taskId);
  if (!control) {
    json(res, { error: `no active worker for ${taskId}` }, 404);
    return;
  }

  control.steer(message);
  json(res, { ok: true });
}

function handleAbortTask(taskId: string, _req: IncomingMessage, res: ServerResponse): void {
  const control = app.runtime.getControl(taskId);
  if (!control) {
    json(res, { error: `no active worker for ${taskId}` }, 404);
    return;
  }

  control.abort();
  json(res, { ok: true });
}

const server = createServer(async (req, res) => {
  const method = req.method || 'GET';
  const { segments } = parseRoute(req.url || '/');

  if (method === 'OPTIONS') {
    cors(res);
    res.writeHead(204);
    res.end();
    return;
  }

  try {
    if (method === 'POST' && segments[0] === 'chat' && segments.length === 1) {
      await handleChat(req, res);
      return;
    }

    if (method === 'GET' && segments[0] === 'tasks' && segments.length === 1) {
      handleListTasks(req, res);
      return;
    }

    if (method === 'GET' && segments[0] === 'tasks' && segments.length === 2) {
      handleGetTask(segments[1], req, res);
      return;
    }

    if (method === 'POST' && segments[0] === 'tasks' && segments[2] === 'steer' && segments.length === 3) {
      await handleSteerTask(segments[1], req, res);
      return;
    }

    if (method === 'POST' && segments[0] === 'tasks' && segments[2] === 'abort' && segments.length === 3) {
      handleAbortTask(segments[1], req, res);
      return;
    }

    json(res, { error: 'not found' }, 404);
  } catch (error: unknown) {
    const msg = error instanceof Error ? error.message : String(error);
    json(res, { error: msg }, 500);
  }
});

server.listen(port, () => {
  console.log(`picoagent server listening on http://localhost:${port}`);
  console.log(`control: ${app.controlDir}`);
  console.log(`repo: ${app.runWorkspace.repoDir} (${app.runWorkspace.mode})`);
  console.log(`tasks: ${app.runWorkspace.tasksDir}`);
  console.log('Endpoints:');
  console.log('  POST /chat              - send message (SSE streaming)');
  console.log('  GET  /tasks             - list all tasks for this server process');
  console.log('  GET  /tasks/:id         - get task details');
  console.log('  POST /tasks/:id/steer   - redirect a worker');
  console.log('  POST /tasks/:id/abort   - cancel a worker');
});
