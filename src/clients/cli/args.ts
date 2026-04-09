import { AgentPresetId } from '../../core/types.js';

export type CliCommand =
  | { type: 'help' }
  | { type: 'serve'; hostname: string; port: number }
  | { type: 'run'; agent: AgentPresetId; prompt?: string };

function parseAgent(value: string): AgentPresetId {
  if (value === 'ask' || value === 'exec') {
    return value;
  }

  throw new Error(`Unsupported agent: ${value}`);
}

export function parseCliArgs(argv: string[]): CliCommand {
  if (argv.length === 0 || argv[0] === 'help' || argv[0] === '--help' || argv[0] === '-h') {
    return { type: 'help' };
  }

  if (argv[0] === 'serve') {
    let hostname = '127.0.0.1';
    let port = 4096;

    for (let index = 1; index < argv.length; index += 1) {
      const current = argv[index];
      if (current === '--hostname') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error('--hostname requires a value');
        }
        hostname = value;
        index += 1;
        continue;
      }

      if (current.startsWith('--hostname=')) {
        hostname = current.slice('--hostname='.length);
        continue;
      }

      if (current === '--port') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error('--port requires a value');
        }
        const parsed = Number(value);
        if (!Number.isInteger(parsed) || parsed <= 0) {
          throw new Error(`Invalid port: ${value}`);
        }
        port = parsed;
        index += 1;
        continue;
      }

      if (current.startsWith('--port=')) {
        const value = current.slice('--port='.length);
        const parsed = Number(value);
        if (!Number.isInteger(parsed) || parsed <= 0) {
          throw new Error(`Invalid port: ${value}`);
        }
        port = parsed;
        continue;
      }

      throw new Error(`Unknown serve argument: ${current}`);
    }

    return { type: 'serve', hostname, port };
  }

  if (argv[0] === 'run') {
    let agent: AgentPresetId = 'ask';
    const promptParts: string[] = [];

    for (let index = 1; index < argv.length; index += 1) {
      const current = argv[index];
      if (current === '--agent' || current === '-a') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error(`${current} requires a value`);
        }

        agent = parseAgent(value);
        index += 1;
        continue;
      }

      if (current.startsWith('--agent=')) {
        agent = parseAgent(current.slice('--agent='.length));
        continue;
      }

      promptParts.push(current);
    }

    return {
      type: 'run',
      agent,
      prompt: promptParts.length > 0 ? promptParts.join(' ') : undefined,
    };
  }

  throw new Error(`Unknown command: ${argv[0]}`);
}

export function usage(): string {
  return [
    'Usage:',
    '  pico serve [--hostname <host>] [--port <port>]',
    '  pico run [--agent ask|exec] <prompt>',
    '  pico help',
  ].join('\n');
}
