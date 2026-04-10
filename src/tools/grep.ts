import { z } from 'zod';
import type { NamespaceLikePath } from '../core/file-view.js';
import { Tool } from '../core/types.js';
import { resolveSessionPath } from '../fs/filesystem.js';
import { parseNamespacePath } from './namespace-path.js';

const GrepParams = z.object({
  query: z.string().min(1).describe('Case-insensitive literal text to search for.'),
  path: z
    .string()
    .optional()
    .describe('Optional namespace-rooted prefix filter, e.g. /workspace/src or /session/runs.'),
  context: z.number().int().min(0).max(20).optional().describe('Optional number of surrounding lines to include around each match.'),
  limit: z.number().int().positive().max(500).optional().describe('Maximum number of matches to return.'),
});

export const grepTool: Tool<typeof GrepParams> = {
  name: 'grep',
  description: 'Search text under a namespace path.',
  kind: 'search',
  parameters: GrepParams,
  title: (args) => `Grep ${args.query}`,
  async execute(args, context) {
    const matches = await context.fileView.grep(args.query, {
      path: (args.path ?? '/workspace') as NamespaceLikePath,
      context: args.context,
      limit: args.limit ?? 50,
    });

    return {
      content:
        matches.length > 0
          ? matches.map((match) =>
              match.kind === 'context'
                ? `${match.path}-${match.line}- ${match.text}`
                : `${match.path}:${match.line}: ${match.text}`)
            .join('\n')
          : 'No matches found.',
      rawOutput: {
        query: args.query,
        context: args.context ?? 0,
        count: matches.length,
      },
      locations:
        matches
          .filter((match) => match.kind !== 'context')
          .flatMap((match) => {
            const parsed = parseNamespacePath(match.path);
            if (parsed.namespace !== 'workspace') {
              return [];
            }

            return [{
              path: resolveSessionPath(parsed.relativePath, context.cwd, context.roots),
              line: match.line,
            }];
          }),
    };
  },
};
