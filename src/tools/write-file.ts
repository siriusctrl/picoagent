import { z } from 'zod';
import { Tool, ToolContext, ToolResult } from '../core/types.js';
import fs from 'fs/promises';
import path from 'path';

const WriteFileParams = z.object({
  path: z.string().describe('Path to file'),
  content: z.string().describe('File content')
});

export const writeFileTool: Tool<typeof WriteFileParams> = {
  name: 'write_file',
  description: 'Write/create files',
  parameters: WriteFileParams,
  async execute(args, context: ToolContext): Promise<ToolResult> {
    try {
      const fullPath = path.resolve(context.cwd, args.path);

      // Enforce write boundary if set
      if (context.writeRoot) {
        const resolvedRoot = path.resolve(context.writeRoot);
        if (!fullPath.startsWith(resolvedRoot + path.sep) && fullPath !== resolvedRoot) {
          return {
            content: `Write denied: path ${fullPath} is outside allowed directory ${resolvedRoot}`,
            isError: true
          };
        }
      }

      await fs.mkdir(path.dirname(fullPath), { recursive: true });
      await fs.writeFile(fullPath, args.content, 'utf-8');
      return { content: `Successfully wrote to ${fullPath}` };
    } catch (error: unknown) {
      return { content: `Error writing file: ${error instanceof Error ? error.message : String(error)}`, isError: true };
    }
  }
};
