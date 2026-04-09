import { z } from 'zod';
import { Tool } from '../core/types.js';
import { resolveSessionPath } from '../fs/filesystem.js';

const CmdParams = z.object({
  target: z.enum(['workspace', 'session']).describe('Which target to execute against.'),
  command: z.string().min(1).describe('Shell command to run with bash -lc.'),
  cwd: z.string().optional().describe('Optional target-relative working directory for the command.'),
});

export const cmdTool: Tool<typeof CmdParams> = {
  name: 'cmd',
  description: 'Run a shell command against a target that supports execution.',
  kind: 'execute',
  parameters: CmdParams,
  title: (args) => `Cmd ${args.target}:${args.command}`,
  locations: (args, context) =>
    args.target === 'workspace' && args.cwd
      ? [{ path: resolveSessionPath(args.cwd, context.cwd, context.roots) }]
      : [],
  async execute(args, context) {
    const result = await context.fileView.cmd(args.target, {
      command: 'bash',
      args: ['-lc', args.command],
      cwd: args.cwd,
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
