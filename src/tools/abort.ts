import { z } from "zod";
import { Tool } from "../core/types.js";
import { writeSignal, updateTaskStatus } from "../core/task.js";
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

    // Write signal first, in case update fails or takes time?
    // Usually signals are picked up by worker loop.
    // If worker is running, it will check signal.
    // But we are setting status to aborted immediately here.
    // If worker is running, it might overwrite status back to running/completed?
    // Typically worker checks signal, then aborts itself.
    // But here we set status too.
    // The prompt says: "Writes an abort signal file and updates task status to 'aborted'".
    // So I do both.

    try {
      writeSignal(taskDir, "abort");
      updateTaskStatus(taskDir, "aborted");
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
