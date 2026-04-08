import { z } from 'zod';
import { Tool } from '../core/types.js';
import { relativeToCwd, resolveSessionPath } from '../lib/filesystem.js';

const SearchTextParams = z.object({
  query: z.string().min(1).describe('Case-insensitive text to search for.'),
  path: z.string().optional().describe('Optional relative directory to search from.'),
  limit: z.number().int().positive().max(200).optional().describe('Maximum number of matches to return.'),
});

export const searchTextTool: Tool<typeof SearchTextParams> = {
  name: 'search_text',
  description: 'Search for text across workspace files.',
  kind: 'search',
  parameters: SearchTextParams,
  title: (args) => `Search for "${args.query}"`,
  locations: (args, context) => [{ path: resolveSessionPath(args.path ?? '.', context.cwd, context.roots) }],
  async execute(args, context) {
    const root = resolveSessionPath(args.path ?? '.', context.cwd, context.roots);
    const matches = await context.environment.searchText(root, args.query, args.limit ?? 50, context.signal);

    const content = matches
      .map((match) => `${relativeToCwd(match.path, context.cwd)}:${match.line}: ${match.text}`)
      .join('\n');

    return {
      content: content || 'No matches found.',
      rawOutput: { query: args.query, count: matches.length },
      locations: matches.slice(0, 20).map((match) => ({ path: match.path, line: match.line })),
    };
  },
};
