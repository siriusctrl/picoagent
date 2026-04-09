import { z } from 'zod';
import { Tool } from '../core/types.js';
import { resolveSessionPath } from '../fs/filesystem.js';

const WriteFileParams = z.object({
  path: z.string().describe('Relative path to the file to write.'),
  content: z.string().describe('Full file content to write.'),
});

export const writeFileTool: Tool<typeof WriteFileParams> = {
  name: 'write_file',
  description: 'Write a text file in the workspace.',
  kind: 'edit',
  parameters: WriteFileParams,
  title: (args) => `Write ${args.path}`,
  locations: (args, context) => [{ path: resolveSessionPath(args.path, context.cwd, context.roots) }],
  async execute(args, context) {
    const fullPath = resolveSessionPath(args.path, context.cwd, context.roots);

    let oldText: string | undefined;
    try {
      oldText = await context.environment.readTextFile(context.sessionId, fullPath);
    } catch {
      oldText = undefined;
    }

    await context.environment.writeTextFile(context.sessionId, fullPath, args.content);

    return {
      content: oldText === undefined ? `Created ${args.path}` : `Updated ${args.path}`,
      display: [{ type: 'diff', path: fullPath, oldText, newText: args.content }],
      rawOutput: { path: fullPath, created: oldText === undefined },
      locations: [{ path: fullPath }],
    };
  },
};
