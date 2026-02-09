import { Tool, ToolContext, Message, AssistantMessage } from '../core/types.js';
import { Provider } from '../core/provider.js';
import { runAgentLoop } from '../core/loop.js';
import { runWorker, WorkerResult } from './worker.js';
import { Tracer } from '../lib/tracer.js';
import { WorkerControl, createWorkerControlHooks } from './worker-control.js';
import { createTraceHooks } from '../hooks/tracing.js';
import { AgentHooks, combineHooks } from '../core/hooks.js';
import { CompactionConfig, createCompactionHooks, DEFAULT_CONFIG } from '../hooks/compaction.js';

export class Runtime {
  private mainMessages: Message[] = [];
  private activeWorkers = new Map<string, WorkerControl>();

  constructor(
    private provider: Provider,
    private mainTools: Tool[],
    private workerTools: Tool[],
    private context: ToolContext,
    private systemPrompt?: string,
    private traceDir?: string,
    private compactionConfig: CompactionConfig = DEFAULT_CONFIG
  ) {}

  async onUserMessage(
    input: string,
    onTextDelta?: (text: string) => void
  ): Promise<AssistantMessage> {
    this.mainMessages.push({ role: 'user', content: input });

    let hooks: AgentHooks = createCompactionHooks(this.provider, this.compactionConfig);

    if (this.traceDir) {
        const tracer = new Tracer(this.traceDir);
        hooks = combineHooks(hooks, createTraceHooks(tracer, this.provider.model));
    }

    if (onTextDelta) {
        hooks = combineHooks(hooks, { onTextDelta });
    }

    const result = await runAgentLoop(
      this.mainMessages,
      this.mainTools,
      this.provider,
      this.context,
      this.systemPrompt,
      hooks
    );

    return result;
  }

  getControl(taskId: string): WorkerControl | undefined {
      return this.activeWorkers.get(taskId);
  }

  spawnWorker(taskDir: string): void {
    const taskId = taskDir.split('/').pop() || 'unknown';
    
    const control = new WorkerControl();
    
    // Setup hooks for worker
    let hooks = createWorkerControlHooks(control, taskId);
    
    // Add compaction hooks
    hooks = combineHooks(hooks, createCompactionHooks(this.provider, this.compactionConfig));
    
    if (this.traceDir) {
        const tracer = new Tracer(this.traceDir); 
        hooks = combineHooks(hooks, createTraceHooks(tracer, this.provider.model));
    }
    
    this.activeWorkers.set(taskId, control);

    runWorker(
      taskDir,
      this.workerTools,
      this.provider,
      this.context,
      hooks
    )
      .then((result) => {
        this.activeWorkers.delete(taskId);
        const msg = `[Task ${result.taskId} completed. Status: ${result.status}]\n` +
          (result.result ? `Result: ${result.result}` : `Error: ${result.error}`);
        
        // Inject notification
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
