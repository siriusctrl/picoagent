import { z } from 'zod';
import type { NamespaceLikePath } from '../core/file-view.ts';
import { Tool } from '../core/types.ts';
import { resolveSessionPath } from '../fs/filesystem.ts';
import { parseNamespacePath } from './namespace-path.ts';

const CmdParams = z.object({
  command: z.string().min(1).describe('Shell command to run with bash -lc.'),
  cwd: z
    .string()
    .min(1)
    .describe('Required namespace path working directory for the command, e.g. /workspace, /sandbox, or /workspace/src.'),
});

export const cmdTool: Tool<typeof CmdParams> = {
  name: 'cmd',
  description: 'Run a shell command in a namespace path that supports cmd.',
  kind: 'execute',
  parameters: CmdParams,
  title: (args) => `Cmd ${args.command}`,
  locations: (args, context) => {
    if (!args.cwd) {
      return [];
    }

    const parsed = parseNamespacePath(args.cwd);
    if (parsed.namespace !== 'workspace') {
      return [];
    }

    return [{ path: resolveSessionPath(parsed.relativePath, context.cwd, context.roots) }];
  },
  async execute(args, context) {
    const result = await context.fileView.cmd({
      command: 'bash',
      args: ['-lc', args.command],
      cwd: args.cwd as NamespaceLikePath,
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
