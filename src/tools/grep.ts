import { z } from 'zod';
import { Tool } from '../core/types.js';

type FileViewTarget = 'workspace' | 'session';

function parseNamespacePath(value: string): { target: FileViewTarget; path?: string } {
  if (!value.startsWith('/')) {
    throw new Error(`Expected a namespace path, for example '/workspace/src'.`);
  }

  const [, namespace, ...parts] = value.split('/');
  if (!namespace) {
    throw new Error(`Expected a namespace path, for example '/workspace/src'.`);
  }

  const normalized = namespace.split('@').at(-1);
  if (normalized !== 'workspace' && normalized !== 'session') {
    throw new Error(`Unsupported namespace '${namespace}'.`);
  }

  const suffix = parts.join('/');
  return {
    target: normalized,
    path: suffix ? suffix : undefined,
  };
}

function namespacePath(target: FileViewTarget, relativePath: string): string {
  if (relativePath.startsWith('/')) {
    return relativePath;
  }

  if (relativePath === '.') {
    return `/${target}`;
  }

  return `/${target}/${relativePath}`;
}

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
    const parsedPath = args.path ? parseNamespacePath(args.path) : undefined;
    const matches = await context.fileView.grep(parsedPath?.target ?? 'workspace', args.query, {
      path: parsedPath?.path,
      context: args.context,
      limit: args.limit ?? 50,
    });

    const renderPath = (path: string) =>
      namespacePath(parsedPath?.target ?? 'workspace', path);

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
        query: args.query,
        context: args.context ?? 0,
        count: matches.length,
      },
      locations:
        matches.filter((match) => match.kind !== 'context').map((match) => ({
          path: renderPath(match.path),
          line: match.line,
        })),
    };
  },
};
