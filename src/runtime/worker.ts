import { join } from 'path';
import { readFileSync, writeFileSync } from 'fs';
import { parseFrontmatter } from '../lib/frontmatter.js';
import { updateTaskStatus } from '../lib/task.js';
import { buildWorkerPrompt } from '../lib/prompt.js';
import { runAgentLoop } from '../core/loop.js';
import { Tool, ToolContext } from '../core/types.js';
import { Provider } from '../core/provider.js';
import { AgentHooks } from '../core/hooks.js';

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
  baseContext: ToolContext,
  hooks?: AgentHooks
): Promise<WorkerResult> {
  const taskPath = join(taskDir, 'task.md');
  const content = readFileSync(taskPath, 'utf-8');
  const { frontmatter, body: instructions } = parseFrontmatter(content);
  const taskId = String(frontmatter.id);
  const taskName = String(frontmatter.name);
  const taskDesc = String(frontmatter.description);

  updateTaskStatus(taskDir, 'running');

  const resultPath = join(taskDir, 'result.md');

  const systemPrompt = buildWorkerPrompt(
    taskDir,
    baseContext.cwd,
    instructions,
    taskId,
    taskName,
    taskDesc
  );

  // Worker context: cwd and writeRoot scoped to task directory
  const workerContext: ToolContext = {
    ...baseContext,
    cwd: taskDir,
    writeRoot: taskDir,
  };

  try {
    const resultMsg = await runAgentLoop(
      [{ role: 'user', content: instructions }],
      tools,
      provider,
      workerContext,
      systemPrompt,
      hooks
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
