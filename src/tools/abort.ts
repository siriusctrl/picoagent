import { z } from "zod";
import { Tool } from "../core/types.js";
import { updateTaskStatus } from "../core/task.js";
import { join } from "path";
import { existsSync } from "fs";

const AbortParams = z.object({
  id: z.string().describe("Task ID to abort")
});

export const abortTool: Tool<typeof AbortParams> = {
  name: "abort",
  description: "Abort a running task",
  parameters: AbortParams,
  execute: async (args, context) => {
    const taskDir = join(context.tasksRoot, args.id);

    if (!existsSync(taskDir)) {
      return {
        content: `Task ${args.id} not found.`,
        isError: true
      };
    }

    try {
      updateTaskStatus(taskDir, "aborted");
      if (context.onAbort) {
          context.onAbort(args.id);
      }
    } catch (e) {
      return {
        content: `Failed to abort task ${args.id}: ${(e as Error).message}`,
        isError: true
      };
    }

    return {
      content: `Task ${args.id} aborted.`
    };
  }
};
