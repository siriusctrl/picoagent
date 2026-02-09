import { z } from "zod";
import { Tool } from "../core/types.js";
import { join } from "path";
import { existsSync } from "fs";

const SteerParams = z.object({
  id: z.string().describe("Task ID (e.g. t_001)"),
  message: z.string().describe("Message to redirect the worker")
});

export const steerTool: Tool<typeof SteerParams> = {
  name: "steer",
  description: "Send a message to a running worker to redirect its course",
  parameters: SteerParams,
  execute: async (args, context) => {
    const taskDir = join(context.tasksRoot, args.id);
    
    if (!existsSync(taskDir)) {
      return {
        content: `Task ${args.id} not found.`,
        isError: true
      };
    }

    if (context.onSteer) {
        context.onSteer(args.id, args.message);
        return {
            content: `Signal sent to task ${args.id}: steer`
        };
    } else {
        return {
            content: `Steer capability not available in this context.`,
            isError: true
        }
    }
  }
};
