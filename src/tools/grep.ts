import { z } from 'zod';
import { Tool } from '../core/types.js';
import { relativeToCwd } from '../fs/filesystem.js';

const GrepParams = z.object({
  target: z.enum(['workspace', 'session']).describe('Which file-view to search.'),
  query: z.string().min(1).describe('Case-insensitive text to search for.'),
  path: z.string().optional().describe('Optional target-relative file or directory prefix.'),
  limit: z.number().int().positive().max(500).optional().describe('Maximum number of matches to return.'),
});

export const grepTool: Tool<typeof GrepParams> = {
  name: 'grep',
  description: 'Search text across a workspace or session file-view.',
  kind: 'search',
  parameters: GrepParams,
  title: (args) => `Grep ${args.target}:${args.query}`,
  async execute(args, context) {
    const matches = await context.fileView.grep(args.target, args.query, {
      path: args.path,
      limit: args.limit ?? 50,
    });

    const renderPath = (path: string) =>
      args.target === 'workspace' ? relativeToCwd(path, context.cwd) : path;

    return {
      content:
        matches.length > 0
          ? matches.map((match) => `${renderPath(match.path)}:${match.line}: ${match.text}`).join('\n')
          : 'No matches found.',
      rawOutput: {
        target: args.target,
        query: args.query,
        count: matches.length,
      },
      locations:
        args.target === 'workspace'
          ? matches.slice(0, 20).map((match) => ({ path: match.path, line: match.line }))
          : undefined,
    };
  },
};
