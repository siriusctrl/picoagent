import { z } from "zod";
import { Tool } from "../core/types.js";
import { createTask } from "../core/task.js";

const DispatchParams = z.object({
  name: z.string().describe("Short name for the task"),
  description: z.string().describe("What this task should accomplish"),
  instructions: z.string().describe("Detailed instructions for the worker"),
  model: z.string().optional().describe("Model to use (defaults to current model)"),
  tags: z.array(z.string()).optional().describe("Tags for categorization")
});

export const dispatchTool: Tool<typeof DispatchParams> = {
  name: "dispatch",
  description: "Dispatch a new task to a background worker",
  parameters: DispatchParams,
  execute: async (args, context) => {
    const taskInfo = createTask(context.tasksRoot, {
      name: args.name,
      description: args.description,
      instructions: args.instructions,
      model: args.model,
      tags: args.tags
    });

    return {
      content: `Task ${taskInfo.id} created: ${taskInfo.name}. Worker spawning not yet implemented (v0.5).`
    };
  }
};
