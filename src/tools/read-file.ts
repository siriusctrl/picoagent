import { Tool, ToolContext, ToolResult } from '../core/types.js';
import fs from 'fs/promises';
import path from 'path';

export const readFileTool: Tool = {
  name: 'read_file',
  description: 'Read file contents',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Path to file' }
    },
    required: ['path']
  },
  async execute(args: Record<string, unknown>, { cwd }: ToolContext): Promise<ToolResult> {
    const filePath = args.path;
    if (typeof filePath !== 'string') {
      return { content: 'Error: path must be a string', isError: true };
    }

    try {
      const fullPath = path.resolve(cwd, filePath);
      const content = await fs.readFile(fullPath, 'utf-8');
      return { content };
    } catch (error: any) {
      return { content: `Error reading file: ${error.message}`, isError: true };
    }
  }
};
