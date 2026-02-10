import { createServer, IncomingMessage, ServerResponse } from 'node:http';
import { homedir } from 'os';
import { join } from 'path';
import { readFileSync, existsSync } from 'fs';
import { shellTool } from './tools/shell.js';
import { readFileTool } from './tools/read-file.js';
import { writeFileTool } from './tools/write-file.js';
import { scanTool } from './tools/scan.js';
import { loadTool } from './tools/load.js';
import { dispatchTool } from './tools/dispatch.js';
import { steerTool } from './tools/steer.js';
import { abortTool } from './tools/abort.js';
import { ToolContext } from './core/types.js';
import { AnthropicProvider } from './providers/anthropic.js';
import { buildMainPrompt } from './lib/prompt.js';
import { listTasks, readTask } from './lib/task.js';
import { Runtime } from './runtime/runtime.js';
import { DEFAULT_CONFIG } from './hooks/compaction.js';

// --- Config ---

const apiKey = process.env.ANTHROPIC_API_KEY;
if (!apiKey) {
  console.error('Error: ANTHROPIC_API_KEY environment variable is required');
  process.exit(1);
}

const model = process.env.PICOAGENT_MODEL || 'claude-sonnet-4-20250514';
const port = parseInt(process.env.PICOAGENT_PORT || '3000', 10);
const workspaceDir = process.cwd();

// --- Tools ---

const workerTools = [shellTool, readFileTool, writeFileTool, scanTool, loadTool];
const mainTools = [...workerTools, dispatchTool, steerTool, abortTool];

// --- Prompt ---

const systemPrompt = buildMainPrompt(workspaceDir, mainTools);

// --- Runtime ---

const provider = new AnthropicProvider({ apiKey, model, systemPrompt });

const context: ToolContext = {
  cwd: workspaceDir,
  tasksRoot: join(workspaceDir, '.tasks'),
};

const traceDir = join(homedir(), '.picoagent', 'traces');
const contextWindow = parseInt(process.env.PICOAGENT_CONTEXT_WINDOW || '200000', 10);
const compactionConfig = { ...DEFAULT_CONFIG, contextWindow };

const runtime = new Runtime(provider, mainTools, workerTools, context, systemPrompt, traceDir, compactionConfig);

context.onTaskCreated = (taskDir) => runtime.spawnWorker(taskDir);
context.onSteer = (taskId, message) => runtime.getControl(taskId)?.steer(message);
context.onAbort = (taskId) => runtime.getControl(taskId)?.abort();

// --- HTTP helpers ---

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

// --- Routes ---

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

  // SSE streaming
  cors(res);
  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-cache',
    'Connection': 'keep-alive',
  });

  try {
    const result = await runtime.onUserMessage(message, (delta) => {
      res.write(`data: ${JSON.stringify({ type: 'delta', text: delta })}\n\n`);
    });

    // Extract final text
    const text = result.content
      .filter((b) => b.type === 'text')
      .map((b) => b.text)
      .join('');

    res.write(`data: ${JSON.stringify({ type: 'done', text })}\n\n`);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    res.write(`data: ${JSON.stringify({ type: 'error', error: msg })}\n\n`);
  }
  res.end();
}

function handleListTasks(_req: IncomingMessage, res: ServerResponse): void {
  const tasks = listTasks(context.tasksRoot);
  json(res, { tasks });
}

function handleGetTask(taskId: string, _req: IncomingMessage, res: ServerResponse): void {
  const taskDir = join(context.tasksRoot, taskId);
  if (!existsSync(taskDir)) {
    json(res, { error: `task ${taskId} not found` }, 404);
    return;
  }

  const task = readTask(taskDir);

  // Read optional files
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

  const control = runtime.getControl(taskId);
  if (!control) {
    json(res, { error: `no active worker for ${taskId}` }, 404);
    return;
  }

  control.steer(message);
  json(res, { ok: true });
}

function handleAbortTask(taskId: string, _req: IncomingMessage, res: ServerResponse): void {
  const control = runtime.getControl(taskId);
  if (!control) {
    json(res, { error: `no active worker for ${taskId}` }, 404);
    return;
  }

  control.abort();
  json(res, { ok: true });
}

// --- Server ---

const server = createServer(async (req, res) => {
  const method = req.method || 'GET';
  const { segments } = parseRoute(req.url || '/');

  // CORS preflight
  if (method === 'OPTIONS') {
    cors(res);
    res.writeHead(204);
    res.end();
    return;
  }

  try {
    // POST /chat
    if (method === 'POST' && segments[0] === 'chat' && segments.length === 1) {
      await handleChat(req, res);
      return;
    }

    // GET /tasks
    if (method === 'GET' && segments[0] === 'tasks' && segments.length === 1) {
      handleListTasks(req, res);
      return;
    }

    // GET /tasks/:id
    if (method === 'GET' && segments[0] === 'tasks' && segments.length === 2) {
      handleGetTask(segments[1], req, res);
      return;
    }

    // POST /tasks/:id/steer
    if (method === 'POST' && segments[0] === 'tasks' && segments[2] === 'steer' && segments.length === 3) {
      await handleSteerTask(segments[1], req, res);
      return;
    }

    // POST /tasks/:id/abort
    if (method === 'POST' && segments[0] === 'tasks' && segments[2] === 'abort' && segments.length === 3) {
      handleAbortTask(segments[1], req, res);
      return;
    }

    // 404
    json(res, { error: 'not found' }, 404);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    json(res, { error: msg }, 500);
  }
});

server.listen(port, () => {
  console.log(`picoagent server listening on http://localhost:${port}`);
  console.log('Endpoints:');
  console.log('  POST /chat              — send message (SSE streaming)');
  console.log('  GET  /tasks             — list all tasks');
  console.log('  GET  /tasks/:id         — get task details');
  console.log('  POST /tasks/:id/steer   — redirect a worker');
  console.log('  POST /tasks/:id/abort   — cancel a worker');
});
