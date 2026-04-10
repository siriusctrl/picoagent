import { z } from 'zod';
import { Tool } from '../core/types.js';

type FileViewTarget = 'workspace' | 'session';

function parseNamespacePath(pattern: string): { target: FileViewTarget; pattern: string } {
  if (!pattern.startsWith('/')) {
    throw new Error(`Expected a namespace-rooted pattern, for example '/workspace/**/*.ts'.`);
  }

  const [, namespace, ...parts] = pattern.split('/');
  if (!namespace) {
    throw new Error(`Expected a namespace-rooted pattern, for example '/workspace/**/*.ts'.`);
  }

  const normalized = namespace.split('@').at(-1);
  if (normalized !== 'workspace' && normalized !== 'session') {
    throw new Error(`Unsupported namespace '${namespace}'.`);
  }

  return {
    target: normalized,
    pattern: parts.length ? parts.join('/') : '.',
  };
}

function namespacePath(target: FileViewTarget, relativePath: string): string {
  if (relativePath === '.') {
    return `/${target}`;
  }

  if (relativePath.startsWith('/')) {
    return relativePath;
  }

  return `/${target}/${relativePath}`;
}

const GlobParams = z.object({
  pattern: z
    .string()
    .min(1)
    .describe('Namespace-rooted glob pattern like /workspace/src/**/*.ts or /session/runs/*.md.'),
  limit: z.number().int().positive().max(500).optional().describe('Maximum number of paths to return.'),
});

export const globTool: Tool<typeof GlobParams> = {
  name: 'glob',
  description: 'Find file paths by namespace-rooted glob pattern.',
  kind: 'search',
  parameters: GlobParams,
  title: (args) => `Glob ${args.pattern}`,
  async execute(args, context) {
    const parsed = parseNamespacePath(args.pattern);
    const paths = await context.fileView.glob(parsed.target, parsed.pattern, args.limit ?? 200);

    const namespacedPaths = paths.map((path) => namespacePath(parsed.target, path));

    return {
      content: namespacedPaths.length > 0 ? namespacedPaths.join('\n') : 'No matches found.',
      rawOutput: {
        pattern: args.pattern,
        count: namespacedPaths.length,
      },
    };
  },
};
