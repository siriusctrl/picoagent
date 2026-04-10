import { z } from 'zod';
import type { NamespaceLikePath } from '../core/file-view.js';
import { Tool } from '../core/types.js';

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
    const namespacedPaths = await context.fileView.glob(args.pattern as NamespaceLikePath, args.limit ?? 200);

    return {
      content: namespacedPaths.length > 0 ? namespacedPaths.join('\n') : 'No matches found.',
      rawOutput: {
        pattern: args.pattern,
        count: namespacedPaths.length,
      },
    };
  },
};
