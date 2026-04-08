import * as acp from '@agentclientprotocol/sdk';
import { ChildProcess, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { Readable, Writable } from 'node:stream';
import { fileURLToPath } from 'node:url';
import { SessionModeId } from '../core/types.js';

export type UiEvent =
  | { type: 'status'; text: string }
  | { type: 'mode'; mode: SessionModeId }
  | { type: 'assistant_delta'; text: string }
  | { type: 'tool_call'; toolCallId: string; title: string; status?: string; kind?: string; rawInput?: unknown }
  | { type: 'tool_call_update'; toolCallId: string; title?: string | null; status?: string | null; rawOutput?: unknown; text?: string }
  | { type: 'error'; text: string };

export interface TuiControllerOptions {
  cwd: string;
  onEvent: (event: UiEvent) => void;
}

interface TerminalState {
  id: string;
  process: ChildProcess;
  output: string;
  truncated: boolean;
  outputByteLimit: number;
  exitCode?: number | null;
  signal?: string | null;
  exitPromise: Promise<void>;
  resolveExit: () => void;
  released: boolean;
}

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

function resolveAgentCommand(): { command: string; args: string[] } {
  const currentFile = fileURLToPath(import.meta.url);
  const compiled = currentFile.endsWith('.js');
  const extension = compiled ? 'js' : 'ts';
  const agentPath = path.join(path.dirname(currentFile), '..', 'acp', `main.${extension}`);

  if (compiled) {
    return { command: process.execPath, args: [agentPath] };
  }

  return {
    command: process.platform === 'win32' ? 'npx.cmd' : 'npx',
    args: ['tsx', agentPath],
  };
}

export class TuiController {
  private readonly terminals = new Map<string, TerminalState>();
  private readonly cwd: string;
  private readonly onEvent: (event: UiEvent) => void;
  private agentProcess?: ChildProcess;
  private connection?: acp.ClientSideConnection;
  private sessionId?: string;
  private mode: SessionModeId = 'ask';

  constructor(options: TuiControllerOptions) {
    this.cwd = options.cwd;
    this.onEvent = options.onEvent;
  }

  async start(): Promise<void> {
    const agent = resolveAgentCommand();
    this.agentProcess = spawn(agent.command, agent.args, {
      cwd: this.cwd,
      stdio: ['pipe', 'pipe', 'inherit'],
    });
    if (!this.agentProcess.stdin || !this.agentProcess.stdout) {
      throw new Error('Failed to create ACP stdio pipes');
    }

    const input = Writable.toWeb(this.agentProcess.stdin) as WritableStream<Uint8Array>;
    const output = Readable.toWeb(this.agentProcess.stdout) as unknown as ReadableStream<Uint8Array>;
    const stream = acp.ndJsonStream(input, output);
    this.connection = new acp.ClientSideConnection(() => this, stream);

    const initialized = await this.connection.initialize({
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: {
        fs: {
          readTextFile: true,
          writeTextFile: true,
        },
        terminal: true,
      },
    });

    if (initialized.protocolVersion !== acp.PROTOCOL_VERSION) {
      this.onEvent({ type: 'error', text: `ACP protocol mismatch: ${initialized.protocolVersion}` });
    }

    const session = await this.connection.newSession({
      cwd: this.cwd,
      mcpServers: [],
    });

    this.sessionId = session.sessionId;
    const currentMode = session.modes?.currentModeId;
    if (currentMode === 'ask' || currentMode === 'exec') {
      this.mode = currentMode;
    }

    this.onEvent({ type: 'mode', mode: this.mode });
    this.onEvent({ type: 'status', text: `Connected in ${this.mode} mode` });
  }

  async sendPrompt(text: string): Promise<void> {
    if (!this.connection || !this.sessionId) {
      throw new Error('ACP session is not ready');
    }

    await this.connection.prompt({
      sessionId: this.sessionId,
      prompt: [{ type: 'text', text }],
    });
  }

  async setMode(mode: SessionModeId): Promise<void> {
    if (!this.connection || !this.sessionId || mode === this.mode) {
      return;
    }

    await this.connection.setSessionMode({
      sessionId: this.sessionId,
      modeId: mode,
    });
    this.mode = mode;
    this.onEvent({ type: 'mode', mode });
  }

  async stop(): Promise<void> {
    for (const terminal of this.terminals.values()) {
      if (!terminal.released) {
        terminal.process.kill();
      }
    }

    this.agentProcess?.kill();
  }

  async sessionUpdate(params: acp.SessionNotification): Promise<void> {
    const update = params.update;
    switch (update.sessionUpdate) {
      case 'agent_message_chunk':
        if (update.content.type === 'text') {
          this.onEvent({ type: 'assistant_delta', text: update.content.text });
        }
        break;
      case 'tool_call':
        this.onEvent({
          type: 'tool_call',
          toolCallId: update.toolCallId,
          title: update.title,
          status: update.status,
          kind: update.kind,
          rawInput: update.rawInput,
        });
        break;
      case 'tool_call_update': {
        let text: string | undefined;
        if (update.content) {
          const textBlock = update.content.find((item) => item.type === 'content' && item.content.type === 'text');
          if (textBlock?.type === 'content' && textBlock.content.type === 'text') {
            text = textBlock.content.text;
          }
        }

        this.onEvent({
          type: 'tool_call_update',
          toolCallId: update.toolCallId,
          title: update.title,
          status: update.status,
          rawOutput: update.rawOutput,
          text,
        });
        break;
      }
      case 'current_mode_update':
        if (update.currentModeId === 'ask' || update.currentModeId === 'exec') {
          this.mode = update.currentModeId;
          this.onEvent({ type: 'mode', mode: update.currentModeId });
          this.onEvent({ type: 'status', text: `Switched to ${update.currentModeId} mode` });
        }
        break;
      default:
        break;
    }
  }

  async requestPermission(): Promise<acp.RequestPermissionResponse> {
    return {
      outcome: {
        outcome: 'cancelled',
      },
    };
  }

  async readTextFile(params: acp.ReadTextFileRequest): Promise<acp.ReadTextFileResponse> {
    const content = await fs.readFile(params.path, 'utf8');
    if (!params.line && !params.limit) {
      return { content };
    }

    const lines = content.split(/\r?\n/);
    const start = Math.max((params.line ?? 1) - 1, 0);
    const end = params.limit ? start + params.limit : undefined;
    return { content: lines.slice(start, end).join('\n') };
  }

  async writeTextFile(params: acp.WriteTextFileRequest): Promise<acp.WriteTextFileResponse> {
    await fs.mkdir(path.dirname(params.path), { recursive: true });
    await fs.writeFile(params.path, params.content, 'utf8');
    return {};
  }

  async createTerminal(params: acp.CreateTerminalRequest): Promise<acp.CreateTerminalResponse> {
    const terminalId = randomUUID();
    const child = spawn(params.command, params.args ?? [], {
      cwd: params.cwd ?? this.cwd,
      env: {
        ...process.env,
        ...Object.fromEntries((params.env ?? []).map((entry) => [entry.name, entry.value])),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    if (!child.stdout || !child.stderr) {
      throw new Error('Failed to create terminal pipes');
    }

    let resolveExit = () => {};
    const exitPromise = new Promise<void>((resolve) => {
      resolveExit = resolve;
    });

    const terminal: TerminalState = {
      id: terminalId,
      process: child,
      output: '',
      truncated: false,
      outputByteLimit: params.outputByteLimit ?? 32000,
      exitPromise,
      resolveExit,
      released: false,
    };

    const append = (chunk: Buffer) => {
      const next = trimOutput(terminal.output + chunk.toString('utf8'), terminal.outputByteLimit);
      terminal.output = next.output;
      terminal.truncated = next.truncated;
    };

    child.stdout.on('data', append);
    child.stderr.on('data', append);
    child.on('exit', (code, signal) => {
      terminal.exitCode = code;
      terminal.signal = signal;
      terminal.resolveExit();
    });

    this.terminals.set(terminalId, terminal);
    return { terminalId };
  }

  async terminalOutput(params: acp.TerminalOutputRequest): Promise<acp.TerminalOutputResponse> {
    const terminal = this.requireTerminal(params.terminalId);
    return {
      output: terminal.output,
      truncated: terminal.truncated,
      exitStatus:
        terminal.exitCode !== undefined || terminal.signal !== undefined
          ? {
              exitCode: terminal.exitCode ?? null,
              signal: terminal.signal ?? null,
            }
          : undefined,
    };
  }

  async waitForTerminalExit(params: acp.WaitForTerminalExitRequest): Promise<acp.WaitForTerminalExitResponse> {
    const terminal = this.requireTerminal(params.terminalId);
    await terminal.exitPromise;
    return {
      exitCode: terminal.exitCode ?? null,
      signal: terminal.signal ?? null,
    };
  }

  async killTerminal(params: acp.KillTerminalRequest): Promise<acp.KillTerminalResponse> {
    const terminal = this.requireTerminal(params.terminalId);
    terminal.process.kill();
    await terminal.exitPromise;
    return {};
  }

  async releaseTerminal(params: acp.ReleaseTerminalRequest): Promise<acp.ReleaseTerminalResponse> {
    const terminal = this.requireTerminal(params.terminalId);
    terminal.released = true;
    if (terminal.exitCode === undefined && terminal.signal === undefined) {
      terminal.process.kill();
      await terminal.exitPromise;
    }
    return {};
  }

  private requireTerminal(terminalId: string): TerminalState {
    const terminal = this.terminals.get(terminalId);
    if (!terminal) {
      throw new Error(`Terminal ${terminalId} not found`);
    }

    return terminal;
  }
}
