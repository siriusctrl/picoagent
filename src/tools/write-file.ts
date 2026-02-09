import { Tool, ToolContext, ToolResult } from '../core/types.js';
import fs from 'fs/promises';
import path from 'path';

export const writeFileTool: Tool = {
  name: 'write_file',
  description: 'Write/create files',
  parameters: {
    type: 'object',
    properties: {
      path: { type: 'string', description: 'Path to file' },
      content: { type: 'string', description: 'File content' }
    },
    required: ['path', 'content']
  },
  async execute(args: Record<string, unknown>, { cwd }: ToolContext): Promise<ToolResult> {
    const filePath = args.path;
    const content = args.content;
    
    if (typeof filePath !== 'string') {
      return { content: 'Error: path must be a string', isError: true };
    }
    if (typeof content !== 'string') {
      return { content: 'Error: content must be a string', isError: true };
    }

    try {
      const fullPath = path.resolve(cwd, filePath);
      await fs.mkdir(path.dirname(fullPath), { recursive: true });
      await fs.writeFile(fullPath, content, 'utf-8');
      return { content: `Successfully wrote to ${fullPath}` };
    } catch (error: any) {
      return { content: `Error writing file: ${error.message}`, isError: true };
    }
  }
};
