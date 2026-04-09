import { z } from 'zod';
import { Tool } from '../core/types.js';

const GlobParams = z.object({
  target: z.enum(['workspace', 'session']).describe('Which file-view to search.'),
  pattern: z.string().min(1).describe('Glob pattern like src/**/*.ts or runs/*.md.'),
  limit: z.number().int().positive().max(500).optional().describe('Maximum number of paths to return.'),
});

export const globTool: Tool<typeof GlobParams> = {
  name: 'glob',
  description: 'Find file-view paths by glob pattern in a workspace or session.',
  kind: 'search',
  parameters: GlobParams,
  title: (args) => `Glob ${args.target}:${args.pattern}`,
  async execute(args, context) {
    const paths = await context.fileView.glob(args.target, args.pattern, args.limit ?? 200);

    return {
      content: paths.length > 0 ? paths.join('\n') : 'No matches found.',
      rawOutput: {
        target: args.target,
        pattern: args.pattern,
        count: paths.length,
      },
    };
  },
};
