import { z } from 'zod';
import { Tool } from '../core/types.js';
import { relativeToCwd } from '../fs/filesystem.js';

const GrepParams = z.object({
  target: z.enum(['workspace', 'session']).describe('Which file-view to search.'),
  query: z.string().min(1).describe('Case-insensitive literal text to search for.'),
  path: z.string().optional().describe('Optional target-relative file or directory prefix.'),
  context: z.number().int().min(0).max(20).optional().describe('Optional number of surrounding lines to include around each match.'),
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
      context: args.context,
      limit: args.limit ?? 50,
    });

    const renderPath = (path: string) =>
      args.target === 'workspace' ? relativeToCwd(path, context.cwd) : path;

    return {
      content:
        matches.length > 0
          ? matches.map((match) =>
              match.kind === 'context'
                ? `${renderPath(match.path)}-${match.line}- ${match.text}`
                : `${renderPath(match.path)}:${match.line}: ${match.text}`)
            .join('\n')
          : 'No matches found.',
      rawOutput: {
        target: args.target,
        query: args.query,
        context: args.context ?? 0,
        count: matches.length,
      },
      locations:
        args.target === 'workspace'
          ? matches.filter((match) => match.kind !== 'context').slice(0, 20).map((match) => ({ path: match.path, line: match.line }))
          : undefined,
    };
  },
};
