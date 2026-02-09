import { z } from 'zod';
import { Tool, ToolContext, ToolResult } from '../core/types.js';
import fs from 'fs/promises';
import path from 'path';

const ReadFileParams = z.object({
  path: z.string().describe('Path to file')
});

export const readFileTool: Tool<typeof ReadFileParams> = {
  name: 'read_file',
  description: 'Read file contents',
  parameters: ReadFileParams,
  async execute(args, { cwd }: ToolContext): Promise<ToolResult> {
    try {
      const fullPath = path.resolve(cwd, args.path);
      const content = await fs.readFile(fullPath, 'utf-8');
      return { content };
    } catch (error: unknown) {
      return { content: `Error reading file: ${error instanceof Error ? error.message : String(error)}`, isError: true };
    }
  }
};
