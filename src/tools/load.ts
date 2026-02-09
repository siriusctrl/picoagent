import { z } from "zod";
import { Tool, ToolContext } from "../core/types.js";
import { load } from "../core/scanner.js";
import { join } from "path";

const LoadParams = z.object({
  path: z.string().describe("Path to file to load")
});

export const loadTool: Tool<typeof LoadParams> = {
  name: "load",
  description: "Load a markdown file fully: frontmatter + body.",
  parameters: LoadParams,
  execute: async (args: z.infer<typeof LoadParams>, context: ToolContext) => {
    // Resolve relative path against cwd
    let filePath = args.path;
    if (!filePath.startsWith("/")) {
      filePath = join(context.cwd, filePath);
    }
    
    try {
      const result = load(filePath);
      return {
        content: JSON.stringify(result, null, 2)
      };
    } catch (e: any) {
      return {
        content: `Error loading file ${filePath}: ${e.message}`,
        isError: true
      };
    }
  }
};
