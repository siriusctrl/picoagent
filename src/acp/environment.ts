import * as acp from '@agentclientprotocol/sdk';
import { AgentEnvironment, RunCommandRequest, RunCommandResult, SearchMatch } from '../core/environment.js';
import { searchFiles, walkFiles } from '../fs/filesystem.js';

export class AcpEnvironment implements AgentEnvironment {
  constructor(private readonly connection: acp.AgentSideConnection) {}

  async readTextFile(
    sessionId: string,
    path: string,
    options?: { line?: number; limit?: number },
  ): Promise<string> {
    const response = await this.connection.readTextFile({
      sessionId,
      path,
      line: options?.line,
      limit: options?.limit,
    });

    return response.content;
  }

  async writeTextFile(sessionId: string, path: string, content: string): Promise<void> {
    await this.connection.writeTextFile({ sessionId, path, content });
  }

  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
    return walkFiles(root, limit, signal);
  }

  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]> {
    return searchFiles(root, query, limit, signal);
  }

  async runCommand(request: RunCommandRequest): Promise<RunCommandResult> {
    const terminal = await this.connection.createTerminal({
      sessionId: request.sessionId,
      command: request.command,
      args: request.args,
      cwd: request.cwd,
      outputByteLimit: request.outputByteLimit,
    });

    try {
      const exitStatus = await terminal.waitForExit();
      const output = await terminal.currentOutput();

      return {
        terminalId: terminal.id,
        output: output.output,
        truncated: output.truncated,
        exitCode: exitStatus.exitCode ?? null,
        signal: exitStatus.signal ?? null,
      };
    } finally {
      await terminal.release();
    }
  }
}
