import { randomUUID } from 'node:crypto';
import http from 'node:http';
import { isAbsolute, join, relative } from 'node:path';
import { buildSessionControlSnapshot, computeControlVersion, SessionControlSnapshot } from '../runtime/control-snapshot.js';
import { createRuntimeContext } from '../runtime/index.js';
import type { AgentEnvironment, SearchMatch } from '../core/environment.js';
import { FilePatchChange, FilePatchOperation, FileViewAccess, FileViewTarget } from '../core/file-view.js';
import { runAgentLoop } from '../core/loop.js';
import { AgentPresetId, AssistantMessage, Message } from '../core/types.js';
import { filterGlob, grepTextBlobs, TextBlob } from '../fs/file-view.js';
import { relativeToCwd, resolveSessionPath } from '../fs/filesystem.js';
import { createProvider } from '../providers/index.js';
import { LocalEnvironment } from './local-environment.js';
import { buildOpenApiDocument } from './openapi.js';
import {
  EmittedRunEvent,
  FileRuntimeStore,
  InMemoryRuntimeStore,
  RunEvent,
  RunSnapshot,
  RuntimeStore,
  RunStatus,
  SessionRecord,
  SessionSnapshot,
} from './runtime-store.js';

export interface HttpServerOptions {
  cwd?: string;
  hostname?: string;
  port?: number;
  environment?: AgentEnvironment;
  runtimeRoot?: string;
  persistentRuntime?: boolean;
}

class NotFoundError extends Error {}
class ConflictError extends Error {}
class ValidationError extends Error {}

function nowIso(): string {
  return new Date().toISOString();
}

function assistantText(message: AssistantMessage): string {
  return message.content
    .filter((item): item is { type: 'text'; text: string } => item.type === 'text')
    .map((item) => item.text)
    .join('');
}

async function readJsonBody(request: http.IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }

  if (chunks.length === 0) {
    return {};
  }

  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch (error: unknown) {
    throw new ValidationError(
      error instanceof Error ? `Invalid JSON body: ${error.message}` : 'Invalid JSON body',
    );
  }
}

function sendJson(response: http.ServerResponse, status: number, payload: unknown): void {
  response.statusCode = status;
  response.setHeader('content-type', 'application/json; charset=utf-8');
  response.end(JSON.stringify(payload));
}

function sendText(response: http.ServerResponse, status: number, contentType: string, payload: string): void {
  response.statusCode = status;
  response.setHeader('content-type', contentType);
  response.end(payload);
}

function sendSse(response: http.ServerResponse, event: RunEvent): void {
  response.write(`event: ${event.type}\n`);
  response.write(`data: ${JSON.stringify(event)}\n\n`);
}

function wantsSse(request: http.IncomingMessage): boolean {
  return request.headers.accept?.includes('text/event-stream') ?? false;
}

function parseAgent(value: unknown, fallback: AgentPresetId = 'ask'): AgentPresetId {
  if (value === undefined) {
    return fallback;
  }

  if (value === 'ask' || value === 'exec') {
    return value;
  }

  throw new ValidationError(`Unsupported agent: ${String(value)}`);
}

function requireAgent(value: unknown): AgentPresetId {
  if (value === undefined) {
    throw new ValidationError('agent is required');
  }

  return parseAgent(value);
}

function requirePrompt(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) {
    throw new ValidationError('prompt is required');
  }

  return value;
}

class HttpRuntimeService {
  private readonly runtime;
  private readonly store: RuntimeStore;

  constructor(
    private readonly cwd: string,
    private readonly environment: AgentEnvironment,
    runtimeRoot = join(cwd, '.pico', 'runtime'),
    persistentRuntime = true,
  ) {
    this.runtime = createRuntimeContext(cwd);
    this.store = persistentRuntime
      ? new FileRuntimeStore(runtimeRoot)
      : new InMemoryRuntimeStore();
  }

  createSession(agent: AgentPresetId = 'ask'): SessionRecord {
    const control = buildSessionControlSnapshot(this.cwd, this.runtime.registry);
    const session: SessionRecord = {
      id: randomUUID(),
      cwd: this.cwd,
      roots: [this.cwd],
      agent,
      controlVersion: control.controlVersion,
      controlConfig: control.config,
      systemPrompts: control.systemPrompts,
      createdAt: nowIso(),
      runIds: [],
      messages: [],
      checkpoints: [],
    };
    return this.store.createSession(session);
  }

  getSession(id: string): SessionRecord {
    const session = this.store.getSession(id);
    if (!session) {
      throw new NotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  getRun(id: string) {
    const run = this.store.getRun(id);
    if (!run) {
      throw new NotFoundError(`Run ${id} not found`);
    }

    return run;
  }

  getRunSnapshot(id: string): RunSnapshot {
    const run = this.store.getRunSnapshot(id);
    if (!run) {
      throw new NotFoundError(`Run ${id} not found`);
    }

    return run;
  }

  getSessionSnapshot(id: string): SessionSnapshot {
    const session = this.store.getSessionSnapshot(id);
    if (!session) {
      throw new NotFoundError(`Session ${id} not found`);
    }

    return session;
  }

  createStandaloneRun(prompt: string, agent: AgentPresetId): RunSnapshot {
    const control = buildSessionControlSnapshot(this.cwd, this.runtime.registry);
    const run = this.createRun(prompt, agent);
    this.startRun(run, control);
    return this.getRunSnapshot(run.id);
  }

  createSessionRun(sessionId: string, prompt: string, agent?: AgentPresetId): RunSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    const control = this.ensureSessionControlSnapshot(session);
    const run = this.createRun(prompt, agent ?? session.agent, session.id);
    this.store.attachRunToSession(session.id, run.id);
    this.startRun(run, control, session);
    return this.getRunSnapshot(run.id);
  }

  setSessionAgent(sessionId: string, agent: AgentPresetId): SessionSnapshot {
    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    this.store.setSessionAgent(sessionId, agent);
    return this.getSessionSnapshot(sessionId);
  }

  getRunEvents(runId: string): { runId: string; status: RunStatus; events: RunEvent[] } {
    const events = this.store.getRunEvents(runId);
    if (!events) {
      throw new NotFoundError(`Run ${runId} not found`);
    }

    return events;
  }

  subscribeToRun(runId: string, listener: (event: RunEvent) => void): () => void {
    const unsubscribe = this.store.subscribeToRun(runId, listener);
    if (!unsubscribe) {
      throw new NotFoundError(`Run ${runId} not found`);
    }

    return unsubscribe;
  }

  listSessionResources(sessionId: string, path = '.'): string[] {
    this.getSession(sessionId);
    const entries = this.store.listSessionResources(sessionId, path);
    if (!entries) {
      throw new NotFoundError(`Session resource directory not found: ${path}`);
    }

    return entries;
  }

  readSessionResource(sessionId: string, path: string): string {
    this.getSession(sessionId);
    const content = this.store.readSessionResource(sessionId, path);
    if (content === undefined) {
      throw new NotFoundError(`Session resource not found: ${path}`);
    }

    return content;
  }

  listSessionFileView(sessionId: string): string[] {
    const session = this.getSession(sessionId);
    return [
      'summary.md',
      ...session.checkpoints.map((checkpoint) => `checkpoints/${checkpoint.id}.md`),
      ...session.runIds.map((runId) => `runs/${runId}.md`),
    ];
  }

  readSessionFileView(sessionId: string, path: string): string {
    const normalized = path.replace(/^\/+|\/+$/g, '');
    if (!this.listSessionFileView(sessionId).includes(normalized)) {
      throw new NotFoundError(`Session file-view path not found: ${path}`);
    }

    return this.readSessionResource(sessionId, normalized);
  }

  compactSession(sessionId: string, keepLastMessages = 8) {
    if (!Number.isInteger(keepLastMessages) || keepLastMessages < 0) {
      throw new ValidationError('keepLastMessages must be a non-negative integer');
    }

    const session = this.getSession(sessionId);
    if (session.activeRunId) {
      throw new ConflictError(`Session ${sessionId} already has an active run`);
    }

    const result = this.store.compactSession(sessionId, keepLastMessages);
    if (!result) {
      throw new NotFoundError(`Session ${sessionId} not found`);
    }

    return {
      checkpoint: result,
      session: this.getSessionSnapshot(sessionId),
    };
  }

  private fileView(
    runId: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
  ): FileViewAccess {
    return {
      glob: async (target, pattern, limit) => this.globFileView(target, pattern, cwd, roots, signal, sessionId, limit),
      grep: async (target, query, options) => this.grepFileView(target, query, runId, cwd, roots, signal, sessionId, options),
      read: async (target, path, options) => this.readFileView(target, path, runId, cwd, roots, sessionId, options),
      patch: async (target, operations) => this.patchFileView(target, operations, runId, cwd, roots),
      cmd: async (target, request) => this.cmdFileView(target, request, runId, cwd, roots),
    };
  }

  private async globFileView(
    target: FileViewTarget,
    pattern: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
    limit = 200,
  ): Promise<string[]> {
    const paths = target === 'workspace'
      ? await this.listWorkspaceFileViewPaths(cwd, roots, signal)
      : this.listSessionFileViewPathsOrThrow(sessionId);

    return filterGlob(paths, pattern, limit);
  }

  private async grepFileView(
    target: FileViewTarget,
    query: string,
    runId: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
    options?: { path?: string; limit?: number; context?: number },
  ): Promise<SearchMatch[]> {
    if (target === 'workspace') {
      const ripgrepMatches = await this.tryGrepWorkspaceWithRipgrep(runId, cwd, roots, query, options);
      if (ripgrepMatches) {
        return ripgrepMatches;
      }
    }

    const blobs = target === 'workspace'
      ? await this.readWorkspaceFileViewBlobs(runId, cwd, roots, signal, options?.path)
      : this.readSessionFileViewBlobs(this.requireSessionId(sessionId), options?.path);

    return grepTextBlobs(blobs, query, options?.limit ?? 50, options?.context ?? 0);
  }

  private async readFileView(
    target: FileViewTarget,
    path: string,
    runId: string,
    cwd: string,
    roots: string[],
    sessionId?: string,
    options?: { line?: number; limit?: number },
  ): Promise<string> {
    if (target === 'workspace') {
      const fullPath = resolveSessionPath(path, cwd, roots);
      return this.environment.readTextFile(runId, fullPath, options);
    }

    const sessionContent = this.readSessionFileView(this.requireSessionId(sessionId), path);
    if (!options?.line && !options?.limit) {
      return sessionContent;
    }

    const lines = sessionContent.split(/\r?\n/);
    const start = Math.max((options.line ?? 1) - 1, 0);
    const end = options.limit ? start + options.limit : undefined;
    return lines.slice(start, end).join('\n');
  }

  private async patchFileView(
    target: FileViewTarget,
    operations: FilePatchOperation[],
    runId: string,
    cwd: string,
    roots: string[],
  ): Promise<FilePatchChange[]> {
    if (target !== 'workspace') {
      throw new ValidationError('patch is only supported for the workspace target');
    }

    const state = new Map<string, { exists: boolean; content: string }>();

    for (const operation of operations) {
      const fullPath = resolveSessionPath(operation.path, cwd, roots);
      if (!state.has(fullPath)) {
        try {
          state.set(fullPath, {
            exists: true,
            content: await this.environment.readTextFile(runId, fullPath),
          });
        } catch {
          state.set(fullPath, {
            exists: false,
            content: '',
          });
        }
      }

      const current = state.get(fullPath)!;
      if (operation.type === 'create') {
        if (current.exists) {
          throw new ValidationError(`File already exists: ${operation.path}`);
        }

        state.set(fullPath, {
          exists: true,
          content: operation.content,
        });
        continue;
      }

      if (operation.type === 'delete') {
        if (!current.exists) {
          throw new ValidationError(`File not found: ${operation.path}`);
        }

        state.set(fullPath, {
          exists: false,
          content: current.content,
        });
        continue;
      }

      if (!current.exists) {
        throw new ValidationError(`File not found: ${operation.path}`);
      }

      if (!current.content.includes(operation.oldText)) {
        throw new ValidationError(`Text not found in ${operation.path}`);
      }

      const nextContent = operation.all
        ? current.content.split(operation.oldText).join(operation.newText)
        : current.content.replace(operation.oldText, operation.newText);
      state.set(fullPath, {
        exists: true,
        content: nextContent,
      });
    }

    const changes: FilePatchChange[] = [];
    for (const operation of operations) {
      const fullPath = resolveSessionPath(operation.path, cwd, roots);
      const finalState = state.get(fullPath)!;

      if (changes.some((change) => change.path === fullPath)) {
        continue;
      }

      let oldText = '';
      try {
        oldText = await this.environment.readTextFile(runId, fullPath);
      } catch {
        oldText = '';
      }

      if (!finalState.exists) {
        await this.environment.deleteTextFile(runId, fullPath);
        changes.push({
          path: fullPath,
          action: 'delete',
          oldText,
          newText: '',
        });
        continue;
      }

      await this.environment.writeTextFile(runId, fullPath, finalState.content);
      changes.push({
        path: fullPath,
        action: oldText === '' ? 'create' : 'update',
        oldText: oldText || undefined,
        newText: finalState.content,
      });
    }

    return changes;
  }

  private cmdFileView(
    target: FileViewTarget,
    request: { command: string; args?: string[]; cwd?: string; outputByteLimit?: number },
    runId: string,
    cwd: string,
    roots: string[],
  ) {
    if (target !== 'workspace') {
      throw new ValidationError('cmd is only supported for the workspace target');
    }

    return this.environment.runCommand({
      sessionId: runId,
      command: request.command,
      args: request.args,
      cwd: request.cwd ? resolveSessionPath(request.cwd, cwd, roots) : cwd,
      outputByteLimit: request.outputByteLimit,
    });
  }

  private async tryGrepWorkspaceWithRipgrep(
    runId: string,
    cwd: string,
    roots: string[],
    query: string,
    options?: { path?: string; limit?: number; context?: number },
  ): Promise<SearchMatch[] | null> {
    const requests = this.workspaceRipgrepRequests(cwd, roots, options?.path);
    const limit = options?.limit ?? 50;
    const matches: SearchMatch[] = [];

    try {
      for (const request of requests) {
        const result = await this.environment.runCommand({
          sessionId: runId,
          command: 'rg',
          args: [
            '--json',
            '--line-number',
            '--hidden',
            '-F',
            '-i',
            ...(options?.context ? ['-C', String(options.context)] : []),
            '--',
            query,
            ...(request.searchPath ? [request.searchPath] : []),
          ],
          cwd: request.root,
          outputByteLimit: 256000,
        });

        if (result.exitCode !== 0 && result.exitCode !== 1) {
          return null;
        }

        const parsed = parseRipgrepJsonLines(result.output, request.root, cwd, limit - matches.length);
        matches.push(...parsed);
        if (matches.length >= limit) {
          break;
        }
      }

      return matches;
    } catch {
      return null;
    }
  }

  private requireSessionId(sessionId?: string): string {
    if (!sessionId) {
      throw new ValidationError('session target requires a persistent session');
    }

    return sessionId;
  }

  private listSessionFileViewPathsOrThrow(sessionId?: string): string[] {
    return this.listSessionFileView(this.requireSessionId(sessionId));
  }

  private workspaceRipgrepRequests(
    cwd: string,
    roots: string[],
    pathFilter?: string,
  ): Array<{ root: string; searchPath?: string }> {
    if (!pathFilter) {
      return roots.map((root) => ({ root }));
    }

    const resolved = resolveSessionPath(pathFilter, cwd, roots);
    return roots
      .filter((root) => {
        const candidate = relative(root, resolved);
        return candidate === '' || (!candidate.startsWith('..') && !isAbsolute(candidate));
      })
      .map((root) => ({
        root,
        searchPath: relative(root, resolved) || '.',
      }));
  }

  private async listWorkspaceFileViewPaths(
    cwd: string,
    roots: string[],
    signal: AbortSignal,
  ): Promise<string[]> {
    const seen = new Set<string>();
    const results: string[] = [];

    for (const root of roots) {
      const files = await this.environment.listFiles(root, 5000, signal);
      for (const filePath of files) {
        const relative = relativeToCwd(filePath, cwd);
        if (relative === '.' || seen.has(relative)) {
          continue;
        }

        seen.add(relative);
        results.push(relative);
      }
    }

    return results.sort((left, right) => left.localeCompare(right));
  }

  private async readWorkspaceFileViewBlobs(
    runId: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    pathFilter?: string,
  ): Promise<TextBlob[]> {
    const paths = await this.listWorkspaceFileViewPaths(cwd, roots, signal);
    const selected = pathFilter
      ? paths.filter((candidate) => candidate === pathFilter || candidate.startsWith(`${pathFilter}/`))
      : paths;

    const blobs: TextBlob[] = [];
    for (const relativePath of selected) {
      const fullPath = resolveSessionPath(relativePath, cwd, roots);
      try {
        blobs.push({
          path: fullPath,
          content: await this.environment.readTextFile(runId, fullPath),
        });
      } catch {
        continue;
      }
    }

    return blobs;
  }

  private readSessionFileViewBlobs(sessionId: string, pathFilter?: string): TextBlob[] {
    return this.listSessionFileView(sessionId)
      .filter((candidate) => !pathFilter || candidate === pathFilter || candidate.startsWith(`${pathFilter}/`))
      .map((path) => ({
        path,
        content: this.readSessionFileView(sessionId, path),
      }));
  }

  private createRun(prompt: string, agent: AgentPresetId, sessionId?: string) {
    return this.store.createRun({
      id: randomUUID(),
      sessionId,
      agent,
      prompt,
      status: 'running',
      output: '',
      createdAt: nowIso(),
      events: [],
    });
  }

  private emit(runId: string, event: EmittedRunEvent): void {
    this.store.appendRunEvent(runId, {
      ...event,
      timestamp: nowIso(),
    });
  }

  private ensureSessionControlSnapshot(session: SessionRecord): SessionControlSnapshot {
    const latestVersion = computeControlVersion(session.cwd);
    if (latestVersion === session.controlVersion) {
      return {
        workspaceRoot: session.cwd,
        controlVersion: session.controlVersion,
        config: session.controlConfig,
        systemPrompts: session.systemPrompts,
      };
    }

    const refreshed = buildSessionControlSnapshot(session.cwd, this.runtime.registry, latestVersion);
    this.store.refreshSessionControl(session.id, {
      controlVersion: refreshed.controlVersion,
      controlConfig: refreshed.config,
      systemPrompts: refreshed.systemPrompts,
    });
    return refreshed;
  }

  private startRun(
    run: ReturnType<HttpRuntimeService['createRun']>,
    control: SessionControlSnapshot,
    session?: SessionRecord,
  ): void {
    void this.executeRun(run, control, session).catch(() => {
      // Run failures are captured in the run state and emitted as events.
    });
  }

  private async executeRun(
    run: ReturnType<HttpRuntimeService['createRun']>,
    control: SessionControlSnapshot,
    session?: SessionRecord,
  ): Promise<void> {
    const controller = new AbortController();
    const startedAt = nowIso();
    this.store.updateRun(run.id, { startedAt });
    this.emit(run.id, {
      type: 'run_started',
      runId: run.id,
      sessionId: run.sessionId,
      agent: run.agent,
      prompt: run.prompt,
    });

    const conversation: Message[] = session
      ? [...session.messages, { role: 'user', content: run.prompt }]
      : [{ role: 'user', content: run.prompt }];

    const tools = this.runtime.registry.forAgent(run.agent);
    const systemPrompt = control.systemPrompts[run.agent];
    const provider = createProvider(control.config);

    try {
      const finalMessage = await runAgentLoop(
        conversation,
        tools,
        provider,
        {
          runId: run.id,
          sessionId: session?.id,
          cwd: session?.cwd ?? this.cwd,
          roots: session?.roots ?? [this.cwd],
          controlRoot: control.workspaceRoot,
          agent: run.agent,
          signal: controller.signal,
          fileView: this.fileView(run.id, session?.cwd ?? this.cwd, session?.roots ?? [this.cwd], controller.signal, session?.id),
        },
        systemPrompt,
        {
          onTextDelta: async (text) => {
            const latestRun = this.getRun(run.id);
            this.store.updateRun(run.id, { output: latestRun.output + text });
            this.emit(run.id, {
              type: 'assistant_delta',
              runId: run.id,
              sessionId: run.sessionId,
              text,
            });
          },
          onToolStart: async (toolCall, tool) => {
            this.emit(run.id, {
              type: 'tool_call',
              runId: run.id,
              sessionId: run.sessionId,
              title: tool?.title && typeof tool.title === 'string' ? tool.title : tool?.name ?? toolCall.name,
              toolCallId: toolCall.id,
              status: 'pending',
              kind: tool?.kind,
              rawInput: toolCall.arguments,
            });
          },
          onToolEnd: async (toolCall, _tool, result) => {
            this.emit(run.id, {
              type: 'tool_call_update',
              runId: run.id,
              sessionId: run.sessionId,
              toolCallId: toolCall.id,
              title: result.title,
              status: result.message.isError ? 'failed' : 'completed',
              rawOutput: result.rawOutput,
              text: result.message.content,
            });
          },
        },
      );

      const finishedAt = nowIso();
      this.store.updateRun(run.id, {
        output: assistantText(finalMessage),
        status: 'completed',
        finishedAt,
      });
      this.emit(run.id, {
        type: 'done',
        runId: run.id,
        sessionId: run.sessionId,
        output: assistantText(finalMessage),
      });

      if (session) {
        this.store.finishSessionRun(session.id, run.id, conversation);
      }
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      this.store.updateRun(run.id, {
        status: 'failed',
        error: message,
        finishedAt: nowIso(),
      });
      this.emit(run.id, {
        type: 'error',
        runId: run.id,
        sessionId: run.sessionId,
        message,
      });
    } finally {
      if (session) {
        this.store.clearSessionActiveRun(session.id, run.id);
      }
    }
  }
}

function parseRipgrepJsonLines(
  output: string,
  root: string,
  cwd: string,
  limit: number,
): SearchMatch[] {
  if (limit <= 0 || !output.trim()) {
    return [];
  }

  const matches: SearchMatch[] = [];

  for (const rawLine of output.split(/\r?\n/)) {
    if (!rawLine.trim() || matches.length >= limit) {
      continue;
    }

    let record: Record<string, unknown>;
    try {
      record = JSON.parse(rawLine) as Record<string, unknown>;
    } catch {
      continue;
    }

    const type = record.type;
    if (type !== 'match' && type !== 'context') {
      continue;
    }

    const data = typeof record.data === 'object' && record.data ? record.data as Record<string, unknown> : null;
    const pathRecord = data && typeof data.path === 'object' && data.path ? data.path as Record<string, unknown> : null;
    const pathText = typeof pathRecord?.text === 'string' ? pathRecord.text : null;
    const lineNumber = typeof data?.line_number === 'number' ? data.line_number : null;
    const linesRecord = data && typeof data.lines === 'object' && data.lines ? data.lines as Record<string, unknown> : null;
    const lineText = typeof linesRecord?.text === 'string' ? linesRecord.text.replace(/\r?\n$/, '') : null;

    if (!pathText || !lineNumber || lineText === null) {
      continue;
    }

    matches.push({
      path: relativeToCwd(join(root, pathText), cwd),
      line: lineNumber,
      text: lineText,
      kind: type,
    });
  }

  return matches;
}

export async function startHttpServer(options: HttpServerOptions = {}): Promise<http.Server> {
  const hostname = options.hostname ?? '127.0.0.1';
  const port = options.port ?? 4096;
  const cwd = options.cwd ?? process.cwd();
  const service = new HttpRuntimeService(
    cwd,
    options.environment ?? new LocalEnvironment(),
    options.runtimeRoot ?? join(cwd, '.pico', 'runtime'),
    options.persistentRuntime ?? true,
  );

  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? '/', `http://${request.headers.host ?? `${hostname}:${port}`}`);
      const { pathname } = url;

      if (request.method === 'GET' && pathname === '/openapi.json') {
        sendJson(response, 200, buildOpenApiDocument());
        return;
      }

      if (request.method === 'POST' && pathname === '/runs') {
        const body = (await readJsonBody(request)) as { prompt?: unknown; agent?: unknown };
        const run = service.createStandaloneRun(requirePrompt(body.prompt), parseAgent(body.agent));
        sendJson(response, 202, { runId: run.id, status: run.status });
        return;
      }

      const runMatch = pathname.match(/^\/runs\/([^/]+)$/);
      if (request.method === 'GET' && runMatch) {
        sendJson(response, 200, service.getRunSnapshot(runMatch[1]));
        return;
      }

      const eventsMatch = pathname.match(/^\/events\/([^/]+)$/);
      if (request.method === 'GET' && eventsMatch) {
        const runId = eventsMatch[1];
        if (!wantsSse(request)) {
          sendJson(response, 200, service.getRunEvents(runId));
          return;
        }

        service.getRunSnapshot(runId);

        let ended = false;
        const endStream = () => {
          if (ended) {
            return;
          }

          ended = true;
          if (!response.writableEnded) {
            response.end();
          }
        };

        response.writeHead(200, {
          'content-type': 'text/event-stream; charset=utf-8',
          'cache-control': 'no-cache, no-transform',
          connection: 'keep-alive',
        });

        const keepAlive = setInterval(() => {
          response.write(': keep-alive\n\n');
        }, 15000);

        let unsubscribe = () => {};
        unsubscribe = service.subscribeToRun(runId, (event) => {
          sendSse(response, event);
          if (event.type === 'done' || event.type === 'error') {
            clearInterval(keepAlive);
            unsubscribe();
            endStream();
          }
        });

        request.on('close', () => {
          clearInterval(keepAlive);
          unsubscribe();
          endStream();
        });
        return;
      }

      if (request.method === 'POST' && pathname === '/sessions') {
        const body = (await readJsonBody(request)) as { agent?: unknown };
        const session = service.createSession(parseAgent(body.agent));
        sendJson(response, 201, {
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
        });
        return;
      }

      const sessionMatch = pathname.match(/^\/sessions\/([^/]+)$/);
      if (request.method === 'GET' && sessionMatch) {
        sendJson(response, 200, service.getSessionSnapshot(sessionMatch[1]));
        return;
      }

      const sessionRunMatch = pathname.match(/^\/sessions\/([^/]+)\/runs$/);
      if (request.method === 'POST' && sessionRunMatch) {
        const body = (await readJsonBody(request)) as { prompt?: unknown; agent?: unknown };
        const run = service.createSessionRun(
          sessionRunMatch[1],
          requirePrompt(body.prompt),
          body.agent === undefined ? undefined : parseAgent(body.agent),
        );
        sendJson(response, 202, { runId: run.id, status: run.status, sessionId: run.sessionId });
        return;
      }

      const sessionResourcesMatch = pathname.match(/^\/sessions\/([^/]+)\/resources$/);
      if (request.method === 'GET' && sessionResourcesMatch) {
        const path = url.searchParams.get('path') ?? '.';
        sendJson(response, 200, {
          sessionId: sessionResourcesMatch[1],
          path,
          entries: service.listSessionResources(sessionResourcesMatch[1], path),
        });
        return;
      }

      const sessionResourceMatch = pathname.match(/^\/sessions\/([^/]+)\/resources\/(.+)$/);
      if (request.method === 'GET' && sessionResourceMatch) {
        const resourcePath = decodeURIComponent(sessionResourceMatch[2]);
        sendText(
          response,
          200,
          resourcePath.endsWith('.jsonl')
            ? 'application/x-ndjson; charset=utf-8'
            : 'text/plain; charset=utf-8',
          service.readSessionResource(sessionResourceMatch[1], resourcePath),
        );
        return;
      }

      const sessionAgentMatch = pathname.match(/^\/sessions\/([^/]+)\/agent$/);
      if (request.method === 'POST' && sessionAgentMatch) {
        const body = (await readJsonBody(request)) as { agent?: unknown };
        sendJson(response, 200, service.setSessionAgent(sessionAgentMatch[1], requireAgent(body.agent)));
        return;
      }

      const sessionCompactMatch = pathname.match(/^\/sessions\/([^/]+)\/compact$/);
      if (request.method === 'POST' && sessionCompactMatch) {
        const body = (await readJsonBody(request)) as { keepLastMessages?: unknown };
        const keepLastMessages =
          body.keepLastMessages === undefined
            ? undefined
            : typeof body.keepLastMessages === 'number'
              ? body.keepLastMessages
              : Number.NaN;
        sendJson(response, 200, service.compactSession(sessionCompactMatch[1], keepLastMessages));
        return;
      }

      sendJson(response, 404, { error: 'not found' });
    } catch (error: unknown) {
      if (response.headersSent) {
        if (!response.writableEnded) {
          response.end();
        }
        return;
      }

      if (error instanceof NotFoundError) {
        sendJson(response, 404, { error: error.message });
        return;
      }

      if (error instanceof ConflictError) {
        sendJson(response, 409, { error: error.message });
        return;
      }

      if (error instanceof ValidationError) {
        sendJson(response, 400, { error: error.message });
        return;
      }

      sendJson(response, 500, { error: error instanceof Error ? error.message : String(error) });
    }
  });

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, hostname, () => {
      server.off('error', reject);
      resolve();
    });
  });

  return server;
}
