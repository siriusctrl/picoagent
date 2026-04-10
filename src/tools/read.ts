import { z } from 'zod';
import type { NamespaceLikePath } from '../core/file-view.js';
import { Tool } from '../core/types.js';
import { formatLineNumberedText, resolveSessionPath } from '../fs/filesystem.js';
import { parseNamespacePath } from './namespace-path.js';

const ReadParams = z.object({
  path: z
    .string()
    .min(1)
    .describe('Namespace path to read, for example /workspace/src/app.ts or /session/summary.md.'),
  line: z.number().int().positive().optional().describe('Optional starting line number.'),
  limit: z.number().int().positive().max(500).optional().describe('Optional number of lines to read.'),
});

export const readTool: Tool<typeof ReadParams> = {
  name: 'read',
  description: 'Read a file from a namespace path.',
  kind: 'read',
  parameters: ReadParams,
  title: (args) => `Read ${args.path}`,
  locations: (args, context) => {
    const parsed = parseNamespacePath(args.path);
    if (parsed.namespace !== 'workspace') {
      return [];
    }

    return [{ path: resolveSessionPath(parsed.relativePath, context.cwd, context.roots), line: args.line }];
  },
  async execute(args, context) {
    const content = await context.fileView.read(args.path as NamespaceLikePath, {
      line: args.line,
      limit: args.limit,
    });

    return {
      content: formatLineNumberedText(content, args.line ?? 1),
      display: [{ type: 'text', text: content }],
      rawOutput: {
        path: args.path,
      },
    };
  },
};
