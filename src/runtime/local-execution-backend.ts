import type { ExecutionBackend, ExecutionRequest, ExecutionResult } from '../core/execution.ts';

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
    const command = [request.command, ...(request.args ?? [])];
    const child = Bun.spawn(command, {
      cwd: request.cwd,
      env: process.env,
      stdin: 'ignore',
      stdout: 'pipe',
      stderr: 'pipe',
    });

    let output = '';
    let truncated = false;
    const outputByteLimit = request.outputByteLimit ?? 32000;
    const stdoutReader = child.stdout.getReader();
    const stderrReader = child.stderr.getReader();
    const decoders = {
      stdout: new TextDecoder(),
      stderr: new TextDecoder(),
    };

    const append = (chunk: Uint8Array, stream: 'stdout' | 'stderr') => {
      const next = trimOutput(output + decoders[stream].decode(chunk, { stream: true }), outputByteLimit);
      output = next.output;
      truncated = next.truncated;
    };
    const readers = {
      stdout: stdoutReader,
      stderr: stderrReader,
    } as const;
    type StreamName = 'stdout' | 'stderr';
    type ReadChunk = { done: boolean; value?: Uint8Array };
    const pending = new Map<StreamName, Promise<{ stream: StreamName; chunk: ReadChunk }>>();
    const closed = new Set<'stdout' | 'stderr'>();

    const scheduleRead = (stream: StreamName) => {
      if (closed.has(stream) || pending.has(stream)) {
        return;
      }

      pending.set(
        stream,
        readers[stream].read().then((chunk) => ({
          stream,
          chunk: chunk.done ? { done: true } : { done: false, value: chunk.value },
        })),
      );
    };

    scheduleRead('stdout');
    scheduleRead('stderr');

    while (pending.size > 0) {
      const { stream, chunk } = await Promise.race(pending.values());
      pending.delete(stream);

      if (chunk.done || !chunk.value) {
        closed.add(stream);
        continue;
      }

      append(chunk.value, stream);
      scheduleRead(stream);
    }

    const exitCode = await child.exited;

    return {
      terminalId,
      output,
      truncated,
      exitCode,
      signal: child.signalCode,
    };
  }
}
