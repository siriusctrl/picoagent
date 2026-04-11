import type { ExecutionBackend } from '../core/execution.ts';
import {
  FilePatchChange,
  FilePatchOperation,
  FileViewAccess,
  NamespaceLikePath,
} from '../core/file-view.ts';
import type { SearchMatch } from '../core/filesystem.ts';
import { filterGlob, grepTextBlobs, TextBlob } from '../fs/file-view.ts';
import { relativeToCwd, resolveSessionPath } from '../fs/filesystem.ts';
import { Namespace } from '../fs/namespace.ts';
import { isAbsolutePath, relativePath } from '../fs/path.ts';
import { SessionFilesystem } from './session-filesystem.ts';
import type { SessionStore } from './store.ts';

function dataAsRecord(value: unknown): Record<string, unknown> {
  return value as Record<string, unknown>;
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

export interface RuntimeFileViewOptions {
  namespace: Namespace;
  sessionStore: SessionStore;
  executionBackend: ExecutionBackend;
  runId: string;
  cwd: string;
  roots: string[];
  signal: AbortSignal;
  sessionId?: string;
  validationError(message: string): Error;
}

class RuntimeFileView {
  constructor(private readonly options: RuntimeFileViewOptions) {}

  access(): FileViewAccess {
    return {
      glob: async (pattern, limit) => this.glob(pattern, limit),
      grep: async (query, options) => this.grep(query, options),
      read: async (path, options) => this.read(path, options),
      patch: async (operations) => this.patch(operations),
      cmd: async (request) => this.cmd(request),
    };
  }

  private getActiveNamespace(sessionId?: string): Namespace {
    if (!sessionId) {
      return this.options.namespace;
    }

    return new Namespace([
      ...this.options.namespace.listMounts(),
      {
        name: 'session',
        filesystem: new SessionFilesystem(this.options.sessionStore, sessionId),
        root: '.',
        supportsCmd: false,
      },
    ]);
  }

  private requireSessionId(sessionId?: string): string {
    if (!sessionId) {
      throw this.options.validationError('session namespace requires a persistent session');
    }

    return sessionId;
  }

  private resolveNamespacePath(namespacePath: NamespaceLikePath): { mountName: string; relativePath: string } {
    if (namespacePath === '/session' || namespacePath.startsWith('/session/')) {
      this.requireSessionId(this.options.sessionId);
    }

    const namespace = this.getActiveNamespace(this.options.sessionId);
    const parsed = namespace.resolveNamespacePath(namespacePath);
    if (parsed.mountName === 'session') {
      this.requireSessionId(this.options.sessionId);
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

  private resolveFilePath(mountName: string, pathValue: string): string {
    if (mountName === 'workspace') {
      return resolveSessionPath(pathValue, this.options.cwd, this.options.roots);
    }

    return pathValue;
  }

  private resolveCommandPath(namespace: Namespace, mountName: string, relativePath: string): string {
    if (mountName === 'workspace') {
      return resolveSessionPath(relativePath, this.options.cwd, this.options.roots);
    }

    return namespace.resolvePath(mountName, relativePath);
  }

  private async glob(pattern: NamespaceLikePath, limit = 200): Promise<string[]> {
    const resolved = this.resolveNamespacePath(pattern);

    if (resolved.mountName === 'workspace') {
      return filterGlob(await this.listWorkspacePaths(), resolved.relativePath, limit)
        .map((filePath) => this.namespacePath('workspace', filePath));
    }

    const namespace = this.getActiveNamespace(this.requireSessionId(this.options.sessionId));
    return filterGlob(await namespace.listFiles(resolved.mountName, '.', 5000, this.options.signal), resolved.relativePath, limit)
      .map((filePath) => this.namespacePath(resolved.mountName, filePath));
  }

  private async grep(
    query: string,
    options?: { path?: NamespaceLikePath; limit?: number; context?: number },
  ): Promise<SearchMatch[]> {
    const rootPath = options?.path ?? '/workspace';
    const resolved = this.resolveNamespacePath(rootPath);
    const resolvedOptions = {
      ...options,
      path: resolved.relativePath === '.' ? undefined : resolved.relativePath,
    };

    if (resolved.mountName === 'workspace') {
      const ripgrepMatches = await this.tryWorkspaceRipgrep(query, resolvedOptions);
      if (ripgrepMatches) {
        return ripgrepMatches;
      }
    }

    const blobs = resolved.mountName === 'workspace'
      ? await this.readWorkspaceBlobs(resolvedOptions.path)
      : await this.readMountedBlobs(resolved.mountName, this.requireSessionId(this.options.sessionId), resolvedOptions.path);

    return grepTextBlobs(blobs, query, resolvedOptions.limit ?? 50, resolvedOptions.context ?? 0);
  }

  private async read(
    path: NamespaceLikePath,
    readOptions?: { line?: number; limit?: number },
  ): Promise<string> {
    const resolved = this.resolveNamespacePath(path);
    const namespace = this.getActiveNamespace(
      resolved.mountName === 'session' ? this.requireSessionId(this.options.sessionId) : undefined,
    );
    const resolvedPath = this.resolveFilePath(resolved.mountName, resolved.relativePath);
    return namespace.readTextFile(resolved.mountName, resolvedPath, readOptions);
  }

  private async patch(operations: FilePatchOperation[]): Promise<FilePatchChange[]> {
    const namespace = this.getActiveNamespace(this.options.sessionId);
    const parsedOperations = operations.map((operation) => {
      const resolved = this.resolveNamespacePath(operation.path as NamespaceLikePath);
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
      throw this.options.validationError('All patch operations must target the same namespace');
    }

    const targetMount = namespace.mount(target);
    if (targetMount.writable === false) {
      throw this.options.validationError(`patch is not supported for namespace '${target}'`);
    }

    const state = new Map<string, { exists: boolean; content: string }>();

    for (const item of parsedOperations) {
      const fullPath = this.resolveFilePath(target, item.relativePath);
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
          throw this.options.validationError(`File already exists: ${item.relativePath}`);
        }

        state.set(fullPath, { exists: true, content: operation.content });
        continue;
      }

      if (operation.type === 'delete') {
        if (!current.exists) {
          throw this.options.validationError(`File not found: ${item.relativePath}`);
        }

        state.set(fullPath, { exists: false, content: current.content });
        continue;
      }

      if (!current.exists) {
        throw this.options.validationError(`File not found: ${operation.path}`);
      }

      if (!current.content.includes(operation.oldText)) {
        throw this.options.validationError(`Text not found in ${item.relativePath}`);
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
      const fullPath = this.resolveFilePath(target, item.relativePath);
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

  private cmd(request: { command: string; args?: string[]; cwd?: NamespaceLikePath; outputByteLimit?: number }) {
    if (!request.cwd) {
      throw this.options.validationError('cmd requires an explicit cwd namespace path');
    }

    const namespace = this.getActiveNamespace(this.options.sessionId);
    const commandPath = request.cwd;
    const resolved = this.resolveNamespacePath(commandPath);
    const targetMount = namespace.mount(resolved.mountName);
    if (!targetMount.supportsCmd) {
      throw this.options.validationError('cmd is not enabled for this namespace');
    }

    const commandCwd = this.resolveCommandPath(namespace, resolved.mountName, resolved.relativePath);
    return this.options.executionBackend.run({
      runId: this.options.runId,
      command: request.command,
      args: request.args,
      cwd: commandCwd,
      outputByteLimit: request.outputByteLimit,
    });
  }

  private async tryWorkspaceRipgrep(
    query: string,
    options?: { path?: string; limit?: number; context?: number },
  ): Promise<SearchMatch[] | null> {
    const requests = this.workspaceRipgrepRequests(options?.path);
    const limit = options?.limit ?? 50;
    const matches: SearchMatch[] = [];

    try {
      for (const request of requests) {
        const result = await this.options.executionBackend.run({
          runId: this.options.runId,
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
          ...parseRipgrepJsonLines(result.output, request.root, this.options.cwd, limit - matches.length).map((match) => ({
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

  private workspaceRipgrepRequests(pathFilter?: string): Array<{ root: string; searchPath?: string }> {
    if (!pathFilter) {
      return this.options.roots.map((root) => ({ root }));
    }

    const resolved = resolveSessionPath(pathFilter, this.options.cwd, this.options.roots);
    return this.options.roots
      .filter((root) => {
        const candidate = relativePath(root, resolved);
        return candidate === '' || (!candidate.startsWith('..') && !isAbsolutePath(candidate));
      })
      .map((root) => ({
        root,
        searchPath: relativePath(root, resolved) || '.',
      }));
  }

  private async listWorkspacePaths(): Promise<string[]> {
    const seen = new Set<string>();
    const results: string[] = [];

    for (const root of this.options.roots) {
      const files = await this.options.namespace.listFiles('workspace', root, 5000, this.options.signal);
      for (const filePath of files) {
        const relativePath = relativeToCwd(filePath, this.options.cwd);
        if (relativePath === '.' || seen.has(relativePath)) {
          continue;
        }

        seen.add(relativePath);
        results.push(relativePath);
      }
    }

    return results.sort((left, right) => left.localeCompare(right));
  }

  private async readWorkspaceBlobs(pathFilter?: string): Promise<TextBlob[]> {
    const paths = await this.listWorkspacePaths();
    const selected = pathFilter
      ? paths.filter((candidate) => candidate === pathFilter || candidate.startsWith(`${pathFilter}/`))
      : paths;

    const blobs: TextBlob[] = [];
    for (const relativePath of selected) {
      const fullPath = resolveSessionPath(relativePath, this.options.cwd, this.options.roots);
      try {
        blobs.push({
          path: this.namespacePath('workspace', relativePath),
          content: await this.options.namespace.readTextFile('workspace', fullPath),
        });
      } catch {
        continue;
      }
    }

    return blobs;
  }

  private async readMountedBlobs(mountName: string, sessionId: string, pathFilter?: string): Promise<TextBlob[]> {
    const namespace = this.getActiveNamespace(sessionId);
    const paths = await namespace.listFiles(mountName, '.', 5000, this.options.signal);
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
}

export function createRuntimeFileViewAccess(options: RuntimeFileViewOptions): FileViewAccess {
  return new RuntimeFileView(options).access();
}
