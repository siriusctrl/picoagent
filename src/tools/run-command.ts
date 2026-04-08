import { z } from 'zod';
import { Tool } from '../core/types.js';
import { resolveSessionPath } from '../lib/filesystem.js';

const RunCommandParams = z.object({
  command: z.string().min(1).describe('Shell command to run with bash -lc.'),
  cwd: z.string().optional().describe('Optional relative working directory for the command.'),
});

export const runCommandTool: Tool<typeof RunCommandParams> = {
  name: 'run_command',
  description: 'Run a shell command inside the workspace.',
  kind: 'execute',
  parameters: RunCommandParams,
  title: (args) => `Run ${args.command}`,
  locations: (args, context) => [
    { path: resolveSessionPath(args.cwd ?? '.', context.cwd, context.roots) },
  ],
  async execute(args, context) {
    const commandCwd = resolveSessionPath(args.cwd ?? '.', context.cwd, context.roots);
    const result = await context.environment.runCommand({
      sessionId: context.sessionId,
      command: 'bash',
      args: ['-lc', args.command],
      cwd: commandCwd,
      outputByteLimit: 32000,
    });

    const status =
      result.exitCode !== undefined && result.exitCode !== null
        ? `exit code ${result.exitCode}`
        : result.signal
          ? `signal ${result.signal}`
          : 'completed';

    const isError = (result.exitCode ?? 0) !== 0 || result.signal !== null;
    return {
      content: `${result.output}\n\nCommand finished with ${status}.`.trim(),
      display: [{ type: 'terminal', terminalId: result.terminalId }],
      rawOutput: {
        output: result.output,
        exitCode: result.exitCode ?? null,
        signal: result.signal ?? null,
        truncated: result.truncated,
      },
      locations: [{ path: commandCwd }],
      isError,
    };
  },
};
