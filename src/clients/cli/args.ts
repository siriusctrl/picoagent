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

function readFlagValue(argv: string[], index: number, flag: string): { value: string; nextIndex: number } | undefined {
  const current = argv[index];
  if (current === flag) {
    const value = argv[index + 1];
    if (!value) {
      throw new Error(`${flag} requires a value`);
    }

    return { value, nextIndex: index + 1 };
  }

  const prefix = `${flag}=`;
  if (current.startsWith(prefix)) {
    return { value: current.slice(prefix.length), nextIndex: index };
  }

  return undefined;
}

function parseServeArgs(argv: string[]): CliCommand {
  let hostname = '127.0.0.1';
  let port = 4096;
  const mounts: CliMount[] = [];
  let session: string | undefined;

  for (let index = 1; index < argv.length; index += 1) {
    const current = argv[index];
    const hostnameOption = readFlagValue(argv, index, '--hostname');
    if (hostnameOption) {
      hostname = hostnameOption.value;
      index = hostnameOption.nextIndex;
      continue;
    }

    const portOption = readFlagValue(argv, index, '--port');
    if (portOption) {
      port = parseIntPort(portOption.value);
      index = portOption.nextIndex;
      continue;
    }

    const mountOption = readFlagValue(argv, index, '--mount');
    if (mountOption) {
      mounts.push(parseMountSpec(mountOption.value));
      index = mountOption.nextIndex;
      continue;
    }

    const sessionOption = readFlagValue(argv, index, '--session');
    if (sessionOption) {
      session = sessionOption.value;
      index = sessionOption.nextIndex;
      continue;
    }

    throw new Error(`Unknown serve argument: ${current}`);
  }

  return { type: 'serve', hostname, port, mounts, session };
}

function parseFilespaceServeArgs(argv: string[]): CliCommand {
  let hostname = '127.0.0.1';
  let port = 4096;
  let name = 'filespace';
  let root = process.cwd();

  for (let index = 2; index < argv.length; index += 1) {
    const current = argv[index];
    const hostnameOption = readFlagValue(argv, index, '--hostname');
    if (hostnameOption) {
      hostname = hostnameOption.value;
      index = hostnameOption.nextIndex;
      continue;
    }

    const portOption = readFlagValue(argv, index, '--port');
    if (portOption) {
      port = parseIntPort(portOption.value);
      index = portOption.nextIndex;
      continue;
    }

    const nameOption = readFlagValue(argv, index, '--name');
    if (nameOption) {
      name = nameOption.value;
      index = nameOption.nextIndex;
      continue;
    }

    const rootOption = readFlagValue(argv, index, '--root');
    if (rootOption) {
      root = rootOption.value;
      index = rootOption.nextIndex;
      continue;
    }

    throw new Error(`Unknown filespace serve argument: ${current}`);
  }

  return { type: 'filespace-serve', hostname, port, name, root };
}

function parseSessionServeArgs(argv: string[]): CliCommand {
  let hostname = '127.0.0.1';
  let port = 4097;
  let root = process.cwd();

  for (let index = 2; index < argv.length; index += 1) {
    const current = argv[index];
    const hostnameOption = readFlagValue(argv, index, '--hostname');
    if (hostnameOption) {
      hostname = hostnameOption.value;
      index = hostnameOption.nextIndex;
      continue;
    }

    const portOption = readFlagValue(argv, index, '--port');
    if (portOption) {
      port = parseIntPort(portOption.value);
      index = portOption.nextIndex;
      continue;
    }

    const rootOption = readFlagValue(argv, index, '--root');
    if (rootOption) {
      root = rootOption.value;
      index = rootOption.nextIndex;
      continue;
    }

    throw new Error(`Unknown session serve argument: ${current}`);
  }

  return { type: 'session-serve', hostname, port, root };
}

export function parseCliArgs(argv: string[]): CliCommand {
  if (argv.length === 0 || argv[0] === 'help' || argv[0] === '--help' || argv[0] === '-h') {
    return { type: 'help' };
  }

  if (argv[0] === 'serve') {
    return parseServeArgs(argv);
  }

  if (argv[0] === 'filespace' && argv[1] === 'serve') {
    return parseFilespaceServeArgs(argv);
  }

  if (argv[0] === 'session' && argv[1] === 'serve') {
    return parseSessionServeArgs(argv);
  }

  if (argv[0] === 'filespace' || argv[0] === 'session') {
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
