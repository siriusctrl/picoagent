import { spawn } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { AgentEnvironment, RunCommandRequest, RunCommandResult, SearchMatch } from '../core/environment.js';
import { ReadTextFileOptions, LocalWorkspaceFileSystem, WorkspaceFileSystem } from '../fs/workspace-fs.js';

function trimOutput(value: string, byteLimit: number): { output: string; truncated: boolean } {
  const encoded = Buffer.from(value, 'utf8');
  if (encoded.byteLength <= byteLimit) {
    return { output: value, truncated: false };
  }

  return {
    output: encoded.subarray(encoded.byteLength - byteLimit).toString('utf8'),
    truncated: true,
  };
}

export class LocalEnvironment implements AgentEnvironment {
  constructor(private readonly fileSystem: WorkspaceFileSystem = new LocalWorkspaceFileSystem()) {}

  async readTextFile(
    _sessionId: string,
    filePath: string,
    options?: ReadTextFileOptions,
  ): Promise<string> {
    return this.fileSystem.readTextFile(filePath, options);
  }

  async writeTextFile(_sessionId: string, filePath: string, content: string): Promise<void> {
    await this.fileSystem.writeTextFile(filePath, content);
  }

  async deleteTextFile(_sessionId: string, filePath: string): Promise<void> {
    await fs.rm(filePath, { force: true });
  }

  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
    return this.fileSystem.listFiles(root, limit, signal);
  }

  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]> {
    return this.fileSystem.searchText(root, query, limit, signal);
  }

  async runCommand(request: RunCommandRequest): Promise<RunCommandResult> {
    const terminalId = `${request.sessionId}:${Date.now().toString(36)}`;
    const child = spawn(request.command, request.args ?? [], {
      cwd: request.cwd,
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    if (!child.stdout || !child.stderr) {
      throw new Error('Failed to create command pipes');
    }

    let output = '';
    let truncated = false;
    const outputByteLimit = request.outputByteLimit ?? 32000;

    const append = (chunk: Buffer | string) => {
      const next = trimOutput(output + chunk.toString(), outputByteLimit);
      output = next.output;
      truncated = next.truncated;
    };

    child.stdout.on('data', append);
    child.stderr.on('data', append);

    const exit = await new Promise<{ exitCode: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', (exitCode, signal) => resolve({ exitCode, signal }));
    });

    return {
      terminalId,
      output,
      truncated,
      exitCode: exit.exitCode,
      signal: exit.signal,
    };
  }
}
