import { Tool, ToolContext, Message, AssistantMessage } from './types.js';
import { Provider } from './provider.js';
import { runAgentLoopStreaming } from './agent-loop.js';
import { runWorker, WorkerResult } from './worker.js';
import { Tracer } from './trace.js';

export class Runtime {
  private mainMessages: Message[] = [];
  private activeWorkers = new Map<string, Promise<WorkerResult>>();

  constructor(
    private provider: Provider,
    private mainTools: Tool[],
    private workerTools: Tool[],
    private context: ToolContext,
    private systemPrompt?: string,
    private traceDir?: string
  ) {}

  async onUserMessage(
    input: string,
    onTextDelta?: (text: string) => void
  ): Promise<AssistantMessage> {
    this.mainMessages.push({ role: 'user', content: input });

    let tracer: Tracer | undefined;
    if (this.traceDir) {
      tracer = new Tracer(this.traceDir);
    }

    const result = await runAgentLoopStreaming(
      this.mainMessages,
      this.mainTools,
      this.provider,
      this.context,
      this.systemPrompt,
      onTextDelta,
      tracer
    );

    return result;
  }

  spawnWorker(taskDir: string): void {
    const taskId = taskDir.split('/').pop() || 'unknown';
    // console.log(`[Runtime] Spawning worker for task ${taskId}...`);

    const workerPromise = runWorker(
      taskDir,
      this.workerTools,
      this.provider,
      this.context
    );

    this.activeWorkers.set(taskId, workerPromise);

    workerPromise
      .then((result) => {
        this.activeWorkers.delete(taskId);
        const msg = `[Task ${result.taskId} completed. Status: ${result.status}]\n` +
          (result.result ? `Result: ${result.result}` : `Error: ${result.error}`);
        
        // Inject notification
        // We use a default logger if this is a background notification
        this.onUserMessage(msg, (text) => process.stdout.write(text))
            .then(() => process.stdout.write('\n> ')) // Restore prompt
            .catch(console.error);
      })
      .catch((err) => {
        this.activeWorkers.delete(taskId);
        const msg = `[Task ${taskId} failed unexpectedly: ${err instanceof Error ? err.message : String(err)}]`;
        this.onUserMessage(msg, (text) => process.stdout.write(text))
            .then(() => process.stdout.write('\n> '))
            .catch(console.error);
      });
  }
}
