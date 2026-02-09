import { createInterface } from 'readline';
import { homedir } from 'os';
import { join } from 'path';
import { runAgentLoopStreaming } from './core/agent-loop.js';
import { shellTool } from './tools/shell.js';
import { readFileTool } from './tools/read-file.js';
import { writeFileTool } from './tools/write-file.js';
import { scanTool } from './tools/scan.js';
import { loadTool } from './tools/load.js';
import { Message, ToolContext } from './core/types.js';
import { AnthropicProvider } from './providers/anthropic.js';
import { Tracer } from './core/trace.js';
import { scan } from './core/scanner.js';

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

const tools = [shellTool, readFileTool, writeFileTool, scanTool, loadTool];

const context: ToolContext = {
  cwd: process.cwd()
};

const messages: Message[] = [];

const rl = createInterface({
  input: process.stdin,
  output: process.stdout
});

console.log('picoagent v0.3');
console.log('Type "exit" to quit');

function ask() {
  rl.question('> ', async (input) => {
    if (input.trim().toLowerCase() === 'exit') {
      rl.close();
      return;
    }

    try {
      messages.push({ role: 'user', content: input });
      
      const traceDir = join(homedir(), '.picoagent', 'traces');
      const tracer = new Tracer(traceDir);

      const response = await runAgentLoopStreaming(
        messages,
        tools,
        provider,
        context,
        undefined,
        (text) => process.stdout.write(text),
        tracer
      );

      console.log(); // Add a newline after the streamed response
    } catch (error: any) {
      console.error('Error:', error.message || error);
    }
    
    ask();
  });
}

ask();
