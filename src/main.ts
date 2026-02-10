import { createInterface } from 'readline';
import { homedir } from 'os';
import { join } from 'path';
import { shellTool } from './tools/shell.js';
import { readFileTool } from './tools/read-file.js';
import { writeFileTool } from './tools/write-file.js';
import { scanTool } from './tools/scan.js';
import { loadTool } from './tools/load.js';
import { dispatchTool } from './tools/dispatch.js';
import { steerTool } from './tools/steer.js';
import { abortTool } from './tools/abort.js';
import { ToolContext } from './core/types.js';
import { loadConfig } from './lib/config.js';
import { createProvider } from './providers/index.js';
import { buildMainPrompt } from './lib/prompt.js';
import { Runtime } from './runtime/runtime.js';
import { DEFAULT_CONFIG } from './hooks/compaction.js';

const workspaceDir = process.cwd();
const config = loadConfig(workspaceDir);

// --- Tools ---

const workerTools = [
  shellTool,
  readFileTool,
  writeFileTool,
  scanTool,
  loadTool
];

const mainTools = [
  ...workerTools,
  dispatchTool,
  steerTool,
  abortTool
];

// --- Prompt & Provider ---

const systemPrompt = buildMainPrompt(workspaceDir);
const provider = createProvider(config, systemPrompt);

// --- Runtime ---

const context: ToolContext = {
  cwd: workspaceDir,
  tasksRoot: join(workspaceDir, ".tasks")
};

const traceDir = join(homedir(), '.picoagent', 'traces');
const compactionConfig = { ...DEFAULT_CONFIG, contextWindow: config.contextWindow };

const runtime = new Runtime(
  provider,
  mainTools,
  workerTools,
  context,
  systemPrompt,
  traceDir,
  compactionConfig
);

context.onTaskCreated = (taskDir) => runtime.spawnWorker(taskDir);
context.onSteer = (taskId, message) => runtime.getControl(taskId)?.steer(message);
context.onAbort = (taskId) => runtime.getControl(taskId)?.abort();

// --- REPL ---

const rl = createInterface({
  input: process.stdin,
  output: process.stdout
});

console.log(`picoagent v0.6 (${config.provider}/${config.model})`);
console.log('Type "exit" to quit');

function ask() {
  rl.question('> ', async (input) => {
    if (input.trim().toLowerCase() === 'exit') {
      rl.close();
      return;
    }

    try {
      await runtime.onUserMessage(input, (text) => process.stdout.write(text));
      console.log(); 
    } catch (error: any) {
      console.error('Error:', error.message || error);
    }
    
    ask();
  });
}

ask();
