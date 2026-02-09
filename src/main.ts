import { createInterface } from 'readline';
import { runAgentLoop } from './core/agent-loop.js';
import { shellTool } from './tools/shell.js';
import { readFileTool } from './tools/read-file.js';
import { writeFileTool } from './tools/write-file.js';
import { Message, ToolContext } from './core/types.js';
import { AnthropicProvider } from './providers/anthropic.js';

const apiKey = process.env.ANTHROPIC_API_KEY;
if (!apiKey) {
  console.error('Error: ANTHROPIC_API_KEY environment variable is required');
  process.exit(1);
}

const model = process.env.PICOAGENT_MODEL || 'claude-sonnet-4-20250514';

const provider = new AnthropicProvider({
  apiKey,
  model,
  systemPrompt: 'You are a helpful coding assistant.'
});

const tools = [shellTool, readFileTool, writeFileTool];

const context: ToolContext = {
  cwd: process.cwd()
};

const messages: Message[] = [];

const rl = createInterface({
  input: process.stdin,
  output: process.stdout
});

console.log('picoagent v0.2');
console.log('Type "exit" to quit');

function ask() {
  rl.question('> ', async (input) => {
    if (input.trim().toLowerCase() === 'exit') {
      rl.close();
      return;
    }

    try {
      messages.push({ role: 'user', content: input });
      
      const response = await runAgentLoop(messages, tools, provider, context);

      for (const block of response.content) {
        if (block.type === 'text') {
          console.log(block.text);
        }
      }
    } catch (error: any) {
      console.error('Error:', error.message || error);
    }
    
    ask();
  });
}

ask();
