import { test } from 'node:test';
import assert from 'node:assert';
import { shellTool } from '../../src/tools/shell.js';

test('shell tool successful command', async () => {
  const result = await shellTool.execute({ command: 'echo hello' }, { cwd: process.cwd() });
  assert.strictEqual(result.content.trim(), 'hello');
  assert.strictEqual(result.isError, undefined);
});

test('shell tool failed command', async () => {
  const result = await shellTool.execute({ command: 'exit 1' }, { cwd: process.cwd() });
  assert.ok(result.isError);
  assert.ok(result.content.includes('Error: Command failed'));
});

test('shell tool output truncation', async () => {
  // Generate large output using node
  // Using 'node -e' to avoid platform specific 'yes' command issues or similar
  const result = await shellTool.execute({ 
    command: 'node -e "console.log(\'a\'.repeat(33000))"' 
  }, { cwd: process.cwd() });
  
  assert.ok(result.content.includes('chars truncated'));
  assert.ok(result.content.length < 33000);
});
