import { z } from 'zod';
import { Tool, ToolContext, ToolResult } from '../core/types.js';
import { exec } from 'child_process';

function truncate(text: string, maxLength: number = 32000): string {
  if (text.length <= maxLength) return text;
  const keep = maxLength - 100;
  const half = Math.floor(keep / 2);
  const head = text.substring(0, half);
  const tail = text.substring(text.length - half);
  return `${head}\n... [${text.length - keep} chars truncated] ...\n${tail}`;
}

const ShellParams = z.object({
  command: z.string().describe('Command to execute')
});

export const shellTool: Tool<typeof ShellParams> = {
  name: 'shell',
  description: 'Execute shell command',
  parameters: ShellParams,
  execute: async (args, { cwd }: ToolContext): Promise<ToolResult> => {
    return new Promise((resolve) => {
      exec(args.command, { cwd, timeout: 30000 }, (error, stdout, stderr) => {
        let output = '';
        if (stdout) output += truncate(stdout);
        if (stderr) {
          if (output) output += '\nstderr:\n';
          output += truncate(stderr);
        }
        if (error) {
          if (error.killed) {
            output += '\n\nError: Command timed out after 30 seconds.';
            resolve({ content: output, isError: true });
            return;
          }
          output += `\n\nError: ${error.message}`;
          resolve({ content: output, isError: true });
          return;
        }
        resolve({ content: output });
      });
    });
  }
};
