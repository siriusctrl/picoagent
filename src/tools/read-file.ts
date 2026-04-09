import { z } from 'zod';
import { Tool } from '../core/types.js';
import { formatLineNumberedText, resolveSessionPath } from '../fs/filesystem.js';

const ReadFileParams = z.object({
  path: z.string().describe('Relative path to the file to read.'),
  line: z.number().int().positive().optional().describe('Optional 1-based start line.'),
  limit: z.number().int().positive().max(500).optional().describe('Optional number of lines to read.'),
});

export const readFileTool: Tool<typeof ReadFileParams> = {
  name: 'read_file',
  description: 'Read a text file from the workspace.',
  kind: 'read',
  parameters: ReadFileParams,
  title: (args) => `Read ${args.path}`,
  locations: (args, context) => [
    {
      path: resolveSessionPath(args.path, context.cwd, context.roots),
      ...(args.line ? { line: args.line } : {}),
    },
  ],
  async execute(args, context) {
    const fullPath = resolveSessionPath(args.path, context.cwd, context.roots);
    const content = await context.environment.readTextFile(context.runId, fullPath, {
      line: args.line,
      limit: args.limit,
    });

    return {
      content: formatLineNumberedText(content, args.line ?? 1),
      display: [{ type: 'text', text: content }],
      rawOutput: { path: fullPath, line: args.line ?? 1, limit: args.limit ?? null },
      locations: [{ path: fullPath, ...(args.line ? { line: args.line } : {}) }],
    };
  },
};
