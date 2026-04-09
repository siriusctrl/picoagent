export interface SearchMatch {
  path: string;
  line: number;
  text: string;
}

export interface RunCommandRequest {
  sessionId: string;
  command: string;
  args?: string[];
  cwd?: string;
  outputByteLimit?: number;
}

export interface RunCommandResult {
  terminalId: string;
  output: string;
  truncated: boolean;
  exitCode?: number | null;
  signal?: string | null;
}

export interface AgentEnvironment {
  readTextFile(
    sessionId: string,
    path: string,
    options?: { line?: number; limit?: number },
  ): Promise<string>;
  writeTextFile(sessionId: string, path: string, content: string): Promise<void>;
  deleteTextFile(sessionId: string, path: string): Promise<void>;
  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]>;
  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]>;
  runCommand(request: RunCommandRequest): Promise<RunCommandResult>;
}
