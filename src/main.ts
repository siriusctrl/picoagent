import { createInterface } from 'readline';
import { createAppBootstrap } from './app/bootstrap.js';

const rl = createInterface({
  input: process.stdin,
  output: process.stdout,
});

const app = createAppBootstrap(process.cwd(), {
  onBackgroundText: (text) => process.stdout.write(text),
  onBackgroundTurnComplete: () => process.stdout.write('\n> '),
  onBackgroundError: (error) => console.error(error),
});

console.log(`picoagent v0.6 (${app.config.provider}/${app.config.model})`);
console.log(`control: ${app.controlDir}`);
console.log(`repo: ${app.runWorkspace.repoDir} (${app.runWorkspace.mode})`);
console.log(`tasks: ${app.runWorkspace.tasksDir}`);
console.log('Type "exit" to quit');

function ask(): void {
  rl.question('> ', async (input) => {
    if (input.trim().toLowerCase() === 'exit') {
      rl.close();
      return;
    }

    try {
      await app.runtime.onUserMessage(input, (text) => process.stdout.write(text));
      console.log();
    } catch (error: unknown) {
      console.error('Error:', error instanceof Error ? error.message : String(error));
    }

    ask();
  });
}

ask();
