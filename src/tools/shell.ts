import { z } from 'zod';
import { Tool, ToolContext, ToolResult } from '../core/types.js';
import { runSandboxedShell } from '../lib/sandbox.js';

const ShellParams = z.object({
  command: z.string().describe('Command to execute')
});

export const shellTool: Tool<typeof ShellParams> = {
  name: 'shell',
  description: 'Execute shell command (sandboxed for workers when writeRoot is set)',
  parameters: ShellParams,
  execute: async (args, ctx: ToolContext): Promise<ToolResult> => {
    const writeRoot = ctx.writeRoot;

    // Default behavior:
    // - main agent (no writeRoot): run plain shell in ctx.cwd
    // - worker (writeRoot set): attempt bwrap sandbox to restrict writes to writeRoot
    const sandboxEnabled = writeRoot ? (ctx.sandbox?.enabled !== false) : false;

    try {
      const res = await runSandboxedShell({
        command: args.command,
        cwd: ctx.cwd,
        writeRoot: writeRoot ?? ctx.cwd,
        enabled: sandboxEnabled,
        bwrapPath: ctx.sandbox?.bwrapPath,
        hideHome: ctx.sandbox?.hideHome,
        timeoutMs: 30000,
        maxOutputChars: 32000,
      });

      let output = '';
      if (res.stdout) output += res.stdout;
      if (res.stderr) {
        if (output) output += '\nstderr:\n';
        output += res.stderr;
      }
      if (res.timedOut) {
        output += '\n\nError: Command timed out after 30 seconds.';
        return { content: output, isError: true };
      }
      if (res.code && res.code !== 0) {
        output += `\n\nError: Command failed (exit code ${res.code}).`;
        return { content: output, isError: true };
      }

      return { content: output };
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      return { content: `Error: ${msg}`, isError: true };
    }
  }
};
