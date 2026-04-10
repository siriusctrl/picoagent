export interface ExecutionRequest {
  runId: string;
  command: string;
  args?: string[];
  cwd?: string;
  outputByteLimit?: number;
}

export interface ExecutionResult {
  terminalId: string;
  output: string;
  truncated: boolean;
  exitCode?: number | null;
  signal?: string | null;
}

export interface ExecutionBackend {
  run(request: ExecutionRequest): Promise<ExecutionResult>;
}
