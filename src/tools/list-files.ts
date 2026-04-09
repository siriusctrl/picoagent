import { z } from 'zod';
import { Tool } from '../core/types.js';
import { relativeToCwd, resolveSessionPath } from '../fs/filesystem.js';

const ListFilesParams = z.object({
  path: z.string().optional().describe('Optional relative directory to list from. Defaults to the session cwd.'),
  limit: z.number().int().positive().max(500).optional().describe('Maximum number of files to return.'),
});

export const listFilesTool: Tool<typeof ListFilesParams> = {
  name: 'list_files',
  description: 'List files under a workspace directory.',
  kind: 'search',
  parameters: ListFilesParams,
  title: (args) => `List files in ${args.path ?? '.'}`,
  locations: (args, context) => [{ path: resolveSessionPath(args.path ?? '.', context.cwd, context.roots) }],
  async execute(args, context) {
    const root = resolveSessionPath(args.path ?? '.', context.cwd, context.roots);
    const files = await context.environment.listFiles(root, args.limit ?? 200, context.signal);

    return {
      content:
        files.length > 0
          ? files.map((filePath) => relativeToCwd(filePath, context.cwd)).join('\n')
          : 'No files found.',
      rawOutput: { path: root, count: files.length },
      locations: [{ path: root }],
    };
  },
};
