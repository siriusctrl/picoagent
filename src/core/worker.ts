import { join } from 'path';
import { readFileSync, writeFileSync } from 'fs';
import { parseFrontmatter } from './scanner.js';
import { updateTaskStatus } from './task.js';
import { runAgentLoop } from './agent-loop.js';
import { Tool, ToolContext } from './types.js';
import { Provider } from './provider.js';

export interface WorkerResult {
  taskId: string;
  status: 'completed' | 'failed';
  result?: string;
  error?: string;
}

export async function runWorker(
  taskDir: string,
  tools: Tool[],
  provider: Provider,
  baseContext: ToolContext
): Promise<WorkerResult> {
  const taskPath = join(taskDir, 'task.md');
  const content = readFileSync(taskPath, 'utf-8');
  const { frontmatter, body: instructions } = parseFrontmatter(content);
  const taskId = String(frontmatter.id);
  const taskName = String(frontmatter.name);
  const taskDesc = String(frontmatter.description);

  updateTaskStatus(taskDir, 'running');

  const progressPath = join(taskDir, 'progress.md');
  const resultPath = join(taskDir, 'result.md');

  const systemPrompt = `You are a Worker for task ${taskId}: "${taskName}".
Description: ${taskDesc}

Your Goal: Complete the task described in the user message.

Protocol:
1. Update ${progressPath} with your status/plan.
2. Write the final result to ${resultPath}.
3. If you fail, write the error to ${resultPath}.

You have access to files in: ${baseContext.cwd}
You are working in: ${taskDir}
`;

  try {
    const resultMsg = await runAgentLoop(
      [{ role: 'user', content: instructions }],
      tools,
      provider,
      baseContext,
      systemPrompt
    );

    // Extract text result from the assistant's final response
    let resultText = "";
    if (Array.isArray(resultMsg.content)) {
      for (const block of resultMsg.content) {
        if (block.type === 'text') {
          resultText += block.text;
        }
      }
    }

    writeFileSync(resultPath, resultText);
    updateTaskStatus(taskDir, 'completed');

    return {
      taskId,
      status: 'completed',
      result: resultText
    };

  } catch (err) {
    const errorMsg = err instanceof Error ? err.message : String(err);
    writeFileSync(resultPath, `Error: ${errorMsg}`);
    updateTaskStatus(taskDir, 'failed');

    return {
      taskId,
      status: 'failed',
      error: errorMsg
    };
  }
}
