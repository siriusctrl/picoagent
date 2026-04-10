import { z } from 'zod';
import { Tool } from '../core/types.js';

type FileViewTarget = 'workspace' | 'session';

function parseNamespacePath(inputPath: string): { target: FileViewTarget; path: string } {
  if (!inputPath.startsWith('/')) {
    throw new Error(`Expected a namespace path, for example '/workspace'.`);
  }

  const [, namespace, ...parts] = inputPath.split('/');
  if (!namespace) {
    throw new Error(`Expected a namespace path, for example '/workspace'.`);
  }

  const normalized = namespace.split('@').at(-1);
  if (normalized !== 'workspace' && normalized !== 'session') {
    throw new Error(`Unsupported namespace '${namespace}'.`);
  }

  return {
    target: normalized,
    path: parts.length ? parts.join('/') : '.',
  };
}

function namespacePath(target: FileViewTarget, relativePath: string): string {
  if (relativePath === '.') {
    return `/${target}`;
  }

  return `/${target}/${relativePath}`;
}

const CmdParams = z.object({
  command: z.string().min(1).describe('Shell command to run with bash -lc.'),
  cwd: z
    .string()
    .optional()
    .describe('Optional namespace path working directory for the command, e.g. /workspace or /workspace/src.'),
});

export const cmdTool: Tool<typeof CmdParams> = {
  name: 'cmd',
  description: 'Run a shell command in the workspace namespace.',
  kind: 'execute',
  parameters: CmdParams,
  title: (args) => `Cmd ${args.command}`,
  locations: (args) => {
    if (!args.cwd) {
      return [];
    }

    const parsed = parseNamespacePath(args.cwd);
    return [{ path: namespacePath(parsed.target, parsed.path) }];
  },
  async execute(args, context) {
    const cwd = args.cwd ? parseNamespacePath(args.cwd) : null;

    if (cwd && cwd.target !== 'workspace') {
      throw new Error('cmd is only supported in the workspace namespace.');
    }

    const result = await context.fileView.cmd('workspace', {
      command: 'bash',
      args: ['-lc', args.command],
      cwd: cwd?.path,
      outputByteLimit: 32000,
    });

    const status =
      result.exitCode !== undefined && result.exitCode !== null
        ? `exit code ${result.exitCode}`
        : result.signal
          ? `signal ${result.signal}`
          : 'completed';

    return {
      content: `${result.output}\n\nCommand finished with ${status}.`.trim(),
      display: [{ type: 'terminal', terminalId: result.terminalId }],
      rawOutput: {
        output: result.output,
        exitCode: result.exitCode ?? null,
        signal: result.signal ?? null,
        truncated: result.truncated,
      },
      isError: (result.exitCode ?? 0) !== 0 || result.signal !== null,
    };
  },
};
