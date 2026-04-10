export type CliCommand =
  | { type: 'help' }
  | {
      type: 'serve';
      hostname: string;
      port: number;
      mounts: { label: string; source: string }[];
      session?: string;
    }
  | {
      type: 'filespace-serve';
      hostname: string;
      port: number;
      name: string;
      root: string;
    }
  | {
      type: 'session-serve';
      hostname: string;
      port: number;
      root: string;
    };

type CliMount = { label: string; source: string };

function parseIntPort(value: string): number {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`Invalid port: ${value}`);
  }

  return parsed;
}

function parseMountSpec(rawValue: string): CliMount {
  const separator = rawValue.indexOf('=');
  if (separator < 0) {
    throw new Error('--mount requires label=source');
  }

  const label = rawValue.slice(0, separator).trim();
  const source = rawValue.slice(separator + 1).trim();

  if (!label || !source) {
    throw new Error('--mount requires label=source');
  }

  return { label, source };
}

export function parseCliArgs(argv: string[]): CliCommand {
  if (argv.length === 0 || argv[0] === 'help' || argv[0] === '--help' || argv[0] === '-h') {
    return { type: 'help' };
  }

  if (argv[0] === 'serve') {
    let hostname = '127.0.0.1';
    let port = 4096;
    const mounts: CliMount[] = [];
    let session: string | undefined;

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
        port = parseIntPort(value);
        index += 1;
        continue;
      }

      if (current.startsWith('--port=')) {
        port = parseIntPort(current.slice('--port='.length));
        continue;
      }

      if (current === '--mount') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error('--mount requires a value');
        }
        mounts.push(parseMountSpec(value));
        index += 1;
        continue;
      }

      if (current.startsWith('--mount=')) {
        mounts.push(parseMountSpec(current.slice('--mount='.length)));
        continue;
      }

      if (current === '--session') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error('--session requires a value');
        }
        session = value;
        index += 1;
        continue;
      }

      if (current.startsWith('--session=')) {
        session = current.slice('--session='.length);
        continue;
      }

      throw new Error(`Unknown serve argument: ${current}`);
    }

    return { type: 'serve', hostname, port, mounts, session };
  }

  if (argv[0] === 'filespace') {
    if (argv[1] !== 'serve') {
      throw new Error(`Unknown command: ${argv[0]} ${argv[1] ?? ''}`.trim());
    }

    let hostname = '127.0.0.1';
    let port = 4096;
    let name = 'filespace';
    let root = process.cwd();

    for (let index = 2; index < argv.length; index += 1) {
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
        port = parseIntPort(value);
        index += 1;
        continue;
      }

      if (current.startsWith('--port=')) {
        port = parseIntPort(current.slice('--port='.length));
        continue;
      }

      if (current === '--name') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error('--name requires a value');
        }
        name = value;
        index += 1;
        continue;
      }

      if (current.startsWith('--name=')) {
        name = current.slice('--name='.length);
        continue;
      }

      if (current === '--root') {
        const value = argv[index + 1];
        if (!value) {
          throw new Error('--root requires a value');
        }
        root = value;
        index += 1;
        continue;
      }

      if (current.startsWith('--root=')) {
        root = current.slice('--root='.length);
        continue;
      }

      throw new Error(`Unknown filespace serve argument: ${current}`);
    }

    return { type: 'filespace-serve', hostname, port, name, root };
  }

  if (argv[0] === 'session') {
    if (argv[1] === 'serve') {
      let hostname = '127.0.0.1';
      let port = 4097;
      let root = process.cwd();

      for (let index = 2; index < argv.length; index += 1) {
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
          port = parseIntPort(value);
          index += 1;
          continue;
        }

        if (current.startsWith('--port=')) {
          port = parseIntPort(current.slice('--port='.length));
          continue;
        }

        if (current === '--root') {
          const value = argv[index + 1];
          if (!value) {
            throw new Error('--root requires a value');
          }
          root = value;
          index += 1;
          continue;
        }

        if (current.startsWith('--root=')) {
          root = current.slice('--root='.length);
          continue;
        }

        throw new Error(`Unknown session serve argument: ${current}`);
      }

      return { type: 'session-serve', hostname, port, root };
    }

    throw new Error(`Unknown command: ${argv[0]} ${argv[1] ?? ''}`.trim());
  }

  throw new Error(`Unknown command: ${argv[0]}`);
}

export function usage(): string {
  return [
    'Usage:',
    '  pico serve [--hostname <host>] [--port <port>] [--mount <label=source> ...] [--session <url>]',
    '  pico filespace serve [--hostname <host>] [--port <port>] [--name <label>] [--root <path>]',
    '  pico session serve [--hostname <host>] [--port <port>] [--root <path>]',
    '  pico help',
  ].join('\n');
}
