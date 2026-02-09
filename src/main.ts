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
import { AnthropicProvider } from './providers/anthropic.js';
import { scan } from './lib/frontmatter.js';
import { Runtime } from './runtime/runtime.js';
import { DEFAULT_CONFIG } from './hooks/compaction.js';

const apiKey = process.env.ANTHROPIC_API_KEY;
if (!apiKey) {
  console.error('Error: ANTHROPIC_API_KEY environment variable is required');
  process.exit(1);
}

const model = process.env.PICOAGENT_MODEL || 'claude-sonnet-4-20250514';

// Scan for skills
let skillDescriptions = "";
const skillsDir = join(process.cwd(), 'skills');
try {
  const skills = scan(skillsDir);
  if (skills.length > 0) {
    skillDescriptions = "Available skills:\n";
    for (const skill of skills) {
      const name = skill.frontmatter.name as string | undefined;
      const desc = skill.frontmatter.description as string | undefined;
      if (name && desc) {
        skillDescriptions += `- ${name}: ${desc}\n`;
      }
    }
  }
} catch (error) {
  // Skills directory might not exist, ignore
}

const baseSystemPrompt = 'You are a helpful coding assistant.';
const systemPrompt = `${baseSystemPrompt}

${skillDescriptions}
Use scan() and load() to explore skills and other markdown files.`.trim();

const provider = new AnthropicProvider({
  apiKey,
  model,
  systemPrompt
});

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

const context: ToolContext = {
  cwd: process.cwd(),
  tasksRoot: join(process.cwd(), ".tasks")
};

const traceDir = join(homedir(), '.picoagent', 'traces');

const contextWindow = parseInt(process.env.PICOAGENT_CONTEXT_WINDOW || '200000', 10);
const compactionConfig = { ...DEFAULT_CONFIG, contextWindow };

const runtime = new Runtime(
  provider,
  mainTools,
  workerTools,
  context,
  systemPrompt,
  traceDir,
  compactionConfig
);

// Set callback
context.onTaskCreated = (taskDir) => runtime.spawnWorker(taskDir);
context.onSteer = (taskId, message) => runtime.getControl(taskId)?.steer(message);
context.onAbort = (taskId) => runtime.getControl(taskId)?.abort();

const rl = createInterface({
  input: process.stdin,
  output: process.stdout
});

console.log('picoagent v0.5');
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
