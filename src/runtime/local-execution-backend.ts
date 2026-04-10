import { spawn } from 'node:child_process';
import type { ExecutionBackend, ExecutionRequest, ExecutionResult } from '../core/execution.js';

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

export class LocalExecutionBackend implements ExecutionBackend {
  async run(request: ExecutionRequest): Promise<ExecutionResult> {
    const terminalId = `${request.runId}:${Date.now().toString(36)}`;
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
