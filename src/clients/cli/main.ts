#!/usr/bin/env node
import type http from 'node:http';
import { AgentPresetId } from '../../core/types.js';
import { parseCliArgs, usage } from './args.js';
import { startHttpServer } from '../../http/server.js';

async function readPromptFromStdin(): Promise<string> {
  if (process.stdin.isTTY) {
    throw new Error('pico run requires a prompt argument or piped stdin');
  }

  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }

  return Buffer.concat(chunks).toString('utf8').trim();
}

async function resolvePrompt(prompt?: string): Promise<string> {
  if (prompt && prompt.trim()) {
    return prompt;
  }

  return readPromptFromStdin();
}

type RunEvent = {
  type: string;
  text?: string;
  title?: string;
  toolCallId?: string;
  status?: string;
  output?: string;
  message?: string;
};

function getServerUrl(server: http.Server): string {
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Expected an inet server address');
  }

  return `http://127.0.0.1:${address.port}`;
}

function parseSseFrame(frame: string): RunEvent | null {
  const lines = frame.split('\n');
  const dataLines: string[] = [];

  for (const line of lines) {
    if (!line || line.startsWith(':')) {
      continue;
    }

    if (line.startsWith('data:')) {
      dataLines.push(line.slice('data:'.length).trimStart());
    }
  }

  if (dataLines.length === 0) {
    return null;
  }

  return JSON.parse(dataLines.join('\n')) as RunEvent;
}

async function runPrompt(prompt: string, agent: AgentPresetId): Promise<void> {
  let wroteAssistantText = false;
  const server = await startHttpServer({
    cwd: process.cwd(),
    hostname: '127.0.0.1',
    port: 0,
  });

  try {
    const response = await fetch(`${getServerUrl(server)}/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ prompt, agent }),
    });

    if (!response.ok) {
      throw new Error(`Run request failed with status ${response.status}`);
    }

    const created = (await response.json()) as { runId: string };
    const eventsResponse = await fetch(`${getServerUrl(server)}/events/${created.runId}`, {
      headers: { accept: 'text/event-stream' },
    });

    if (!eventsResponse.ok) {
      throw new Error(`Event stream failed with status ${eventsResponse.status}`);
    }

    const reader = eventsResponse.body?.getReader();
    if (!reader) {
      throw new Error('Expected event stream body');
    }

    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }

      buffer += decoder.decode(value, { stream: true });
      let boundary = buffer.indexOf('\n\n');
      while (boundary >= 0) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const event = parseSseFrame(frame);
        if (!event) {
          boundary = buffer.indexOf('\n\n');
          continue;
        }

        switch (event.type) {
        case 'assistant_delta':
          process.stdout.write(event.text ?? '');
          wroteAssistantText = true;
          break;
        case 'tool_call':
          process.stderr.write(`[tool] ${event.title ?? event.toolCallId ?? 'unknown'}\n`);
          break;
        case 'tool_call_update':
          if (event.status === 'failed') {
            process.stderr.write(`[tool] ${event.title ?? event.toolCallId ?? 'unknown'} failed\n`);
          }
          break;
        case 'error':
          process.stderr.write(`error: ${event.message ?? 'unknown error'}\n`);
          throw new Error(event.message ?? 'Run failed');
        case 'done':
          if (!wroteAssistantText && event.output) {
            process.stdout.write(event.output);
            wroteAssistantText = true;
          }
          if (wroteAssistantText) {
            process.stdout.write('\n');
          }
          return;
        default:
          break;
        }

        boundary = buffer.indexOf('\n\n');
      }
    }
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }

        resolve();
      });
    });
  }
}

async function main(): Promise<void> {
  try {
    const command = parseCliArgs(process.argv.slice(2));

    switch (command.type) {
      case 'help':
        process.stdout.write(`${usage()}\n`);
        return;
      case 'serve': {
        await startHttpServer({
          cwd: process.cwd(),
          hostname: command.hostname,
          port: command.port,
        });
        process.stdout.write(`Listening on http://${command.hostname}:${command.port}\n`);
        return;
      }
      case 'run': {
        const prompt = await resolvePrompt(command.prompt);
        if (!prompt.trim()) {
          throw new Error('Prompt cannot be empty');
        }

        await runPrompt(prompt, command.agent);
        return;
      }
    }
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`${message}\n\n${usage()}\n`);
    process.exitCode = 1;
  }
}

void main();
