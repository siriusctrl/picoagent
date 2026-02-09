import { z } from "zod";
import { Tool, ToolContext } from "../core/types.js";
import { scan } from "../lib/frontmatter.js";
import { join } from "path";

const ScanParams = z.object({
  dir: z.string().describe("Directory to scan"),
  pattern: z.record(z.string(), z.string()).optional().describe("Filter by frontmatter fields (supports * wildcards)")
});

const scanTool: Tool<typeof ScanParams> = {
  name: "scan",
  description: "Scan a directory for markdown files and return their frontmatter.",
  parameters: ScanParams,
  execute: async (args: z.infer<typeof ScanParams>, context: ToolContext) => {
    // Resolve relative path against cwd
    let dir = args.dir;
    if (!dir.startsWith("/")) {
      dir = join(context.cwd, dir);
    }
    
    const results = scan(dir, args.pattern);
    return {
      content: JSON.stringify(results, null, 2)
    };
  }
};

export { scanTool };
