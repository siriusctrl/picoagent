import { z } from 'zod';
import { Tool } from '../core/types.js';
import { formatLineNumberedText, resolveSessionPath } from '../fs/filesystem.js';

const ReadParams = z.object({
  target: z.enum(['workspace', 'session']).describe('Which file-view to read from.'),
  path: z.string().min(1).describe('Target-relative path to read.'),
  line: z.number().int().positive().optional().describe('Optional starting line number.'),
  limit: z.number().int().positive().max(500).optional().describe('Optional number of lines to read.'),
});

export const readTool: Tool<typeof ReadParams> = {
  name: 'read',
  description: 'Read a file from a workspace or session file-view.',
  kind: 'read',
  parameters: ReadParams,
  title: (args) => `Read ${args.target}:${args.path}`,
  locations: (args, context) =>
    args.target === 'workspace'
      ? [{ path: resolveSessionPath(args.path, context.cwd, context.roots), line: args.line }]
      : [],
  async execute(args, context) {
    const content = await context.fileView.read(args.target, args.path, {
      line: args.line,
      limit: args.limit,
    });

    return {
      content: formatLineNumberedText(content, args.line ?? 1),
      display: [{ type: 'text', text: content }],
      rawOutput: {
        target: args.target,
        path: args.path,
      },
    };
  },
};
