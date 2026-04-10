import http from 'node:http';
import path from 'node:path';
import { Hono } from 'hono';
import { z } from 'zod';
import type { MutableFilesystem } from '../core/filesystem.js';
import { RootedFilesystem } from '../fs/rooted-fs.js';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.js';
import { startNodeFetchServer } from './node-server.js';

export interface FilespaceInfo {
  name: string;
  writable: boolean;
  root: string;
}

export interface FilespaceServerOptions {
  name: string;
  root: string;
  hostname?: string;
  port?: number;
  filesystem?: MutableFilesystem;
  writable?: boolean;
}

const readRequestSchema = z.object({
  path: z.string().min(1),
  options: z
    .object({
      line: z.number().int().positive().optional(),
      limit: z.number().int().positive().optional(),
    })
    .optional(),
});

const listRequestSchema = z.object({
  root: z.string().min(1),
  limit: z.number().int().positive(),
});

const searchRequestSchema = z.object({
  root: z.string().min(1),
  query: z.string(),
  limit: z.number().int().positive(),
});

const writeRequestSchema = z.object({
  path: z.string().min(1),
  content: z.string(),
});

const deleteRequestSchema = z.object({
  path: z.string().min(1),
});

function parseBody<T>(value: unknown, schema: z.ZodSchema<T>): T {
  const result = schema.safeParse(value);
  if (!result.success) {
    throw new Error(result.error.issues[0]?.message ?? 'Invalid request body');
  }

  return result.data;
}

function toRootedFilesystem(options: FilespaceServerOptions): MutableFilesystem {
  const delegate = options.filesystem ?? new LocalWorkspaceFileSystem();
  return new RootedFilesystem(delegate, path.resolve(options.root));
}

export function createFilespaceApp(options: FilespaceServerOptions) {
  const writable = options.writable ?? true;
  const rootedFilesystem = toRootedFilesystem(options);
  const root = path.resolve(options.root);
  const info: FilespaceInfo = {
    name: options.name,
    writable,
    root,
  };

  const app = new Hono();

  app.onError((error, c) => {
    return c.json({ error: error instanceof Error ? error.message : String(error) }, 400);
  });

  app.get('/info', (c) => c.json(info, 200));

  app.post('/read', async (c) => {
    const body = parseBody(await c.req.json(), readRequestSchema);
    const content = await rootedFilesystem.readTextFile(body.path, body.options);
    return c.json({ content }, 200);
  });

  app.post('/list', async (c) => {
    const body = parseBody(await c.req.json(), listRequestSchema);
    const paths = await rootedFilesystem.listFiles(body.root, body.limit, new AbortController().signal);
    return c.json({ paths }, 200);
  });

  app.post('/search', async (c) => {
    const body = parseBody(await c.req.json(), searchRequestSchema);
    const matches = await rootedFilesystem.searchText(body.root, body.query, body.limit, new AbortController().signal);
    return c.json({ matches }, 200);
  });

  app.post('/write', async (c) => {
    if (!writable) {
      return c.json({ error: 'filespace is read-only' }, 405);
    }

    const body = parseBody(await c.req.json(), writeRequestSchema);
    await rootedFilesystem.writeTextFile(body.path, body.content);
    return c.json({ ok: true }, 200);
  });

  app.post('/delete', async (c) => {
    if (!writable) {
      return c.json({ error: 'filespace is read-only' }, 405);
    }

    const body = parseBody(await c.req.json(), deleteRequestSchema);
    await rootedFilesystem.deleteTextFile(body.path);
    return c.json({ ok: true }, 200);
  });

  return {
    app,
    info,
  };
}

export async function startFilespaceServer(options: FilespaceServerOptions): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4096;
  const { app } = createFilespaceApp(options);
  return startNodeFetchServer(app.fetch, hostname, port);
}
