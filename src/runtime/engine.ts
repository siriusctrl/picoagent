import { randomUUID } from 'node:crypto';
import { isAbsolute, relative } from 'node:path';
import { buildSessionControlSnapshot, computeControlVersion, SessionControlSnapshot } from './control-snapshot.js';
import type { ExecutionBackend } from '../core/execution.js';
import type { MutableFilesystem, SearchMatch } from '../core/filesystem.js';
import { FilePatchChange, FilePatchOperation, FileViewAccess, NamespaceLikePath } from '../core/file-view.js';
import { runAgentLoop } from '../core/loop.js';
import { AgentPresetId, AssistantMessage, Message } from '../core/types.js';
import { filterGlob, grepTextBlobs, TextBlob } from '../fs/file-view.js';
import { relativeToCwd, resolveSessionPath } from '../fs/filesystem.js';
import { Namespace } from '../fs/namespace.js';
import { createProvider } from '../providers/index.js';
import { RuntimeContext } from './index.js';
import { SessionFilesystem } from './session-filesystem.js';
import type {
  EmittedRunEvent,
  RunRecord,
  RunSnapshot,
  RuntimeStore,
  SessionRecord,
} from './store.js';

export class RuntimeConflictError extends Error {
  readonly status = 409;
}

export class RuntimeValidationError extends Error {
  readonly status = 400;
}

function nowIso(): string {
  return new Date().toISOString();
}

function assistantText(message: AssistantMessage): string {
  return message.content
    .filter((item): item is { type: 'text'; text: string } => item.type === 'text')
    .map((item) => item.text)
    .join('');
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

    const data = typeof record.data === 'object' && record.data ? dataAsRecord(record.data) : null;
    const pathRecord = data && typeof data.path === 'object' && data.path ? dataAsRecord(data.path) : null;
    const pathText = typeof pathRecord?.text === 'string' ? pathRecord.text : null;
    const lineNumber = typeof data?.line_number === 'number' ? data.line_number : null;
    const linesRecord = data && typeof data.lines === 'object' && data.lines ? dataAsRecord(data.lines) : null;
    const lineText = typeof linesRecord?.text === 'string' ? linesRecord.text.replace(/\r?\n$/, '') : null;

    if (!pathText || !lineNumber || lineText === null) {
      continue;
    }

    matches.push({
      path: relativeToCwd(`${root}/${pathText}`.replace(/\/+/g, '/'), cwd),
      line: lineNumber,
      text: lineText,
      kind: type,
    });
  }

  return matches;
}

function dataAsRecord(value: unknown): Record<string, unknown> {
  return value as Record<string, unknown>;
}

export interface RuntimeEngineOptions {
  cwd: string;
  filesystem: MutableFilesystem;
  executionBackend: ExecutionBackend;
  runtimeContext: RuntimeContext;
  store: RuntimeStore;
}

export class RuntimeEngine {
  private readonly namespace: Namespace;

  constructor(private readonly options: RuntimeEngineOptions) {
    this.namespace = new Namespace([{
      name: 'workspace',
      filesystem: options.filesystem,
      root: '.',
      writable: true,
      executable: true,
    }]);
  }

  async createSession(agent: AgentPresetId = 'ask'): Promise<SessionRecord> {
    const control = await this.buildControlSnapshot(this.options.cwd);
    const session: SessionRecord = {
      id: randomUUID(),
      cwd: this.options.cwd,
      roots: [this.options.cwd],
      agent,
      controlVersion: control.controlVersion,
      controlConfig: control.config,
      systemPrompts: control.systemPrompts,
      createdAt: nowIso(),
      runIds: [],
      messages: [],
      checkpoints: [],
    };
    return this.options.store.createSession(session);
  }

  async createStandaloneRun(prompt: string, agent: AgentPresetId): Promise<RunSnapshot> {
    const control = await this.buildControlSnapshot(this.options.cwd);
    const run = this.createRun(prompt, agent);
    this.startRun(run, control);
    return this.requireRunSnapshot(run.id);
  }

  async createSessionRun(session: SessionRecord, prompt: string, agent?: AgentPresetId): Promise<RunSnapshot> {
    if (session.activeRunId) {
      throw new RuntimeConflictError(`Session ${session.id} already has an active run`);
    }

    const control = await this.ensureSessionControlSnapshot(session);
    const latestSession = this.options.store.getSession(session.id);
    if (!latestSession) {
      throw new Error(`Session ${session.id} not found`);
    }

    if (latestSession.activeRunId) {
      throw new RuntimeConflictError(`Session ${session.id} already has an active run`);
    }

    const run = this.createRun(prompt, agent ?? latestSession.agent, latestSession.id);
    this.options.store.attachRunToSession(latestSession.id, run.id);
    this.startRun(run, control, latestSession);
    return this.requireRunSnapshot(run.id);
  }

  private requireRunSnapshot(runId: string): RunSnapshot {
    const snapshot = this.options.store.getRunSnapshot(runId);
    if (!snapshot) {
      throw new Error(`Run ${runId} not found`);
    }

    return snapshot;
  }

  private createRun(prompt: string, agent: AgentPresetId, sessionId?: string): RunRecord {
    return this.options.store.createRun({
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

  private async buildControlSnapshot(workspaceRoot: string, controlVersion?: string): Promise<SessionControlSnapshot> {
    return buildSessionControlSnapshot(
      workspaceRoot,
      this.options.runtimeContext.registry,
      this.namespace.mount('workspace').filesystem,
      controlVersion,
    );
  }

  private async ensureSessionControlSnapshot(session: SessionRecord): Promise<SessionControlSnapshot> {
    const latestVersion = await computeControlVersion(session.cwd, this.namespace.mount('workspace').filesystem);
    if (latestVersion === session.controlVersion) {
      return {
        workspaceRoot: session.cwd,
        controlVersion: session.controlVersion,
        config: session.controlConfig,
        systemPrompts: session.systemPrompts,
      };
    }

    const refreshed = await this.buildControlSnapshot(session.cwd, latestVersion);
    this.options.store.refreshSessionControl(session.id, {
      controlVersion: refreshed.controlVersion,
      controlConfig: refreshed.config,
      systemPrompts: refreshed.systemPrompts,
    });
    return refreshed;
  }

  private startRun(run: RunRecord, control: SessionControlSnapshot, session?: SessionRecord): void {
    void this.executeRun(run, control, session).catch(() => {
      // Run failures are captured in the run state and emitted as events.
    });
  }

  private emit(runId: string, event: EmittedRunEvent): void {
    this.options.store.appendRunEvent(runId, {
      ...event,
      timestamp: nowIso(),
    });
  }

  private fileView(
    runId: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
  ): FileViewAccess {
    return {
      glob: async (pattern, limit) => this.globFileView(pattern, cwd, roots, signal, sessionId, limit),
      grep: async (query, options) => this.grepFileView(query, runId, cwd, roots, signal, sessionId, options),
      read: async (path, options) => this.readFileView(path, cwd, roots, sessionId, options),
      patch: async (operations) => this.patchFileView(operations, cwd, roots, sessionId),
      cmd: async (request) => this.cmdFileView(request, runId, cwd, roots, sessionId),
    };
  }

  private getActiveFileViewNamespace(sessionId?: string): Namespace {
    if (!sessionId) {
      return this.namespace;
    }

    return new Namespace([
      this.namespace.mount('workspace'),
      {
        name: 'session',
        filesystem: new SessionFilesystem(this.options.store, sessionId),
        root: '.',
      },
    ]);
  }

  private resolveNamespacePath(
    namespacePath: NamespaceLikePath,
    sessionId?: string,
  ): { mountName: string; relativePath: string } {
    if (namespacePath === '/session' || namespacePath.startsWith('/session/')) {
      this.requireSessionId(sessionId);
    }

    const namespace = this.getActiveFileViewNamespace(sessionId);
    const parsed = namespace.resolveNamespacePath(namespacePath);
    if (parsed.mountName === 'session') {
      this.requireSessionId(sessionId);
    }

    return {
      mountName: parsed.mountName,
      relativePath: parsed.relativePath,
    };
  }

  private namespacePath(mountName: string, relativePath: string): NamespaceLikePath {
    return (
      relativePath === '.' || relativePath === ''
        ? `/${mountName}`
        : `/${mountName}/${relativePath}`
    ) as NamespaceLikePath;
  }

  private resolveFilePath(
    mountName: string,
    pathValue: string,
    cwd: string,
    roots: string[],
  ): string {
    if (mountName === 'workspace') {
      return resolveSessionPath(pathValue, cwd, roots);
    }

    return pathValue;
  }

  private async globFileView(
    pattern: NamespaceLikePath,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
    limit = 200,
  ): Promise<string[]> {
    const resolved = this.resolveNamespacePath(pattern, sessionId);

    if (resolved.mountName === 'workspace') {
      return filterGlob(await this.listWorkspaceFileViewPaths(cwd, roots, signal), resolved.relativePath, limit)
        .map((filePath) => this.namespacePath('workspace', filePath));
    }

    const namespace = this.getActiveFileViewNamespace(this.requireSessionId(sessionId));
    return filterGlob(await namespace.listFiles(resolved.mountName, '.', 5000, signal), resolved.relativePath, limit)
      .map((filePath) => this.namespacePath(resolved.mountName, filePath));
  }

  private async grepFileView(
    query: string,
    runId: string,
    cwd: string,
    roots: string[],
    signal: AbortSignal,
    sessionId?: string,
    options?: { path?: NamespaceLikePath; limit?: number; context?: number },
  ): Promise<SearchMatch[]> {
    const rootPath = options?.path ?? '/workspace';
    const resolved = this.resolveNamespacePath(rootPath, sessionId);
    const resolvedOptions = {
      ...options,
      path: resolved.relativePath === '.' ? undefined : resolved.relativePath,
    };

    if (resolved.mountName === 'workspace') {
      const ripgrepMatches = await this.tryGrepWorkspaceWithRipgrep(runId, cwd, roots, query, resolvedOptions);
      if (ripgrepMatches) {
        return ripgrepMatches;
      }
    }

    const blobs = resolved.mountName === 'workspace'
      ? await this.readWorkspaceFileViewBlobs(cwd, roots, signal, resolvedOptions.path)
      : await this.readMountedFileViewBlobs(resolved.mountName, this.requireSessionId(sessionId), signal, resolvedOptions.path);

    return grepTextBlobs(blobs, query, resolvedOptions.limit ?? 50, resolvedOptions.context ?? 0);
  }

  private async readFileView(
    path: NamespaceLikePath,
    cwd: string,
    roots: string[],
    sessionId?: string,
    options?: { line?: number; limit?: number },
  ): Promise<string> {
    const resolved = this.resolveNamespacePath(path, sessionId);
    const namespace = this.getActiveFileViewNamespace(
      resolved.mountName === 'session' ? this.requireSessionId(sessionId) : undefined,
    );
    const resolvedPath = this.resolveFilePath(resolved.mountName, resolved.relativePath, cwd, roots);
    return namespace.readTextFile(resolved.mountName, resolvedPath, options);
  }

  private async patchFileView(
    operations: FilePatchOperation[],
    cwd: string,
    roots: string[],
    sessionId?: string,
  ): Promise<FilePatchChange[]> {
    const namespace = this.getActiveFileViewNamespace(sessionId);
    const parsedOperations = operations.map((operation) => {
      const resolved = this.resolveNamespacePath(operation.path as NamespaceLikePath, sessionId);
      return {
        operation,
        mountName: resolved.mountName,
        relativePath: resolved.relativePath,
      };
    });

    if (parsedOperations.length === 0) {
      return [];
    }

    const target = parsedOperations[0].mountName;
    if (parsedOperations.some((entry) => entry.mountName !== target)) {
      throw new RuntimeValidationError('All patch operations must target the same namespace');
    }

    const targetMount = namespace.mount(target);

    if (targetMount.writable === false) {
      throw new RuntimeValidationError(`patch is not supported for namespace '${target}'`);
    }

    const state = new Map<string, { exists: boolean; content: string }>();

    for (const item of parsedOperations) {
      const fullPath = this.resolveFilePath(target, item.relativePath, cwd, roots);
      if (!state.has(fullPath)) {
        try {
          state.set(fullPath, {
            exists: true,
            content: await namespace.readTextFile(target, fullPath),
          });
        } catch {
          state.set(fullPath, {
            exists: false,
            content: '',
          });
        }
      }

      const current = state.get(fullPath)!;
      const operation = item.operation;
      if (operation.type === 'create') {
        if (current.exists) {
          throw new RuntimeValidationError(`File already exists: ${item.relativePath}`);
        }

        state.set(fullPath, { exists: true, content: operation.content });
        continue;
      }

      if (operation.type === 'delete') {
        if (!current.exists) {
          throw new RuntimeValidationError(`File not found: ${item.relativePath}`);
        }

        state.set(fullPath, { exists: false, content: current.content });
        continue;
      }

      if (!current.exists) {
        throw new RuntimeValidationError(`File not found: ${operation.path}`);
      }

      if (!current.content.includes(operation.oldText)) {
        throw new RuntimeValidationError(`Text not found in ${item.relativePath}`);
      }

      state.set(fullPath, {
        exists: true,
        content: operation.all
          ? current.content.split(operation.oldText).join(operation.newText)
          : current.content.replace(operation.oldText, operation.newText),
      });
    }

    const changes: FilePatchChange[] = [];
    for (const item of parsedOperations) {
      const fullPath = this.resolveFilePath(target, item.relativePath, cwd, roots);
      const finalState = state.get(fullPath)!;
      if (changes.some((change) => change.path === fullPath)) {
        continue;
      }

      let oldText = '';
      try {
        oldText = await namespace.readTextFile(target, fullPath);
      } catch {
        oldText = '';
      }

      if (!finalState.exists) {
        await namespace.deleteTextFile(target, fullPath);
        changes.push({
          path: this.namespacePath(target, item.relativePath),
          action: 'delete',
          oldText,
          newText: '',
        });
        continue;
      }

      await namespace.writeTextFile(target, fullPath, finalState.content);
      changes.push({
        path: this.namespacePath(target, item.relativePath),
        action: oldText === '' ? 'create' : 'update',
        oldText: oldText || undefined,
        newText: finalState.content,
      });
    }

    return changes;
  }

  private cmdFileView(
    request: { command: string; args?: string[]; cwd?: NamespaceLikePath; outputByteLimit?: number },
    runId: string,
    cwd: string,
    roots: string[],
    sessionId?: string,
  ) {
    const commandPath = request.cwd ?? '/workspace';
    const resolved = this.resolveNamespacePath(commandPath, sessionId);
    const namespace = this.getActiveFileViewNamespace(sessionId);
    const targetMount = namespace.mount(resolved.mountName);
    if (targetMount.executable === false) {
      throw new RuntimeValidationError('cmd is not enabled for this namespace');
    }

    const commandCwd = this.resolveFilePath(resolved.mountName, resolved.relativePath, cwd, roots);

    return this.options.executionBackend.run({
      runId,
      command: request.command,
      args: request.args,
      cwd: commandCwd,
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
        const result = await this.options.executionBackend.run({
          runId,
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

        matches.push(
          ...parseRipgrepJsonLines(result.output, request.root, cwd, limit - matches.length).map((match) => ({
            ...match,
            path: this.namespacePath('workspace', match.path),
          })),
        );
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
      throw new RuntimeValidationError('session namespace requires a persistent session');
    }

    return sessionId;
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
      const files = await this.namespace.listFiles('workspace', root, 5000, signal);
      for (const filePath of files) {
        const relativePath = relativeToCwd(filePath, cwd);
        if (relativePath === '.' || seen.has(relativePath)) {
          continue;
        }

        seen.add(relativePath);
        results.push(relativePath);
      }
    }

    return results.sort((left, right) => left.localeCompare(right));
  }

  private async readWorkspaceFileViewBlobs(
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
          path: this.namespacePath('workspace', relativePath),
          content: await this.namespace.readTextFile('workspace', fullPath),
        });
      } catch {
        continue;
      }
    }

    return blobs;
  }

  private async readMountedFileViewBlobs(
    mountName: string,
    sessionId: string,
    signal: AbortSignal,
    pathFilter?: string,
  ): Promise<TextBlob[]> {
    const namespace = this.getActiveFileViewNamespace(sessionId);
    const paths = await namespace.listFiles(mountName, '.', 5000, signal);
    const selected = pathFilter
      ? paths.filter((candidate) => candidate === pathFilter || candidate.startsWith(`${pathFilter}/`))
      : paths;

    const blobs: TextBlob[] = [];
    for (const filePath of selected) {
      blobs.push({
        path: this.namespacePath(mountName, filePath),
        content: await namespace.readTextFile(mountName, filePath),
      });
    }

    return blobs;
  }

  private async executeRun(run: RunRecord, control: SessionControlSnapshot, session?: SessionRecord): Promise<void> {
    const controller = new AbortController();
    const startedAt = nowIso();
    this.options.store.updateRun(run.id, { startedAt });
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

    const tools = this.options.runtimeContext.registry.forAgent(run.agent);
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
          cwd: session?.cwd ?? this.options.cwd,
          roots: session?.roots ?? [this.options.cwd],
          controlRoot: control.workspaceRoot,
          agent: run.agent,
          signal: controller.signal,
          fileView: this.fileView(run.id, session?.cwd ?? this.options.cwd, session?.roots ?? [this.options.cwd], controller.signal, session?.id),
        },
        systemPrompt,
        {
          onTextDelta: async (text) => {
            const latestRun = this.options.store.getRun(run.id);
            if (!latestRun) {
              return;
            }

            this.options.store.updateRun(run.id, { output: latestRun.output + text });
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

      this.options.store.updateRun(run.id, {
        output: assistantText(finalMessage),
        status: 'completed',
        finishedAt: nowIso(),
      });
      this.emit(run.id, {
        type: 'done',
        runId: run.id,
        sessionId: run.sessionId,
        output: assistantText(finalMessage),
      });

      if (session) {
        this.options.store.finishSessionRun(session.id, run.id, conversation);
      }
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      this.options.store.updateRun(run.id, {
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
        this.options.store.clearSessionActiveRun(session.id, run.id);
      }
    }
  }
}
