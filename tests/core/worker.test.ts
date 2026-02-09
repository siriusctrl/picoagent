import { test, after } from 'node:test';
import assert from 'node:assert';
import { runWorker } from '../../src/runtime/worker.js';
import { MockProvider } from '../helpers/mock-provider.js';
import { join } from 'path';
import { ToolContext } from '../../src/core/types.js';
import { writeFileSync, mkdirSync, rmSync, existsSync, readFileSync } from 'fs';
import { parseFrontmatter } from '../../src/lib/frontmatter.js';

const testDir = join(process.cwd(), 'tests', 'temp-worker');

function setupTask(id: string) {
  const taskDir = join(testDir, id);
  if (!existsSync(testDir)) mkdirSync(testDir, { recursive: true });
  if (existsSync(taskDir)) rmSync(taskDir, { recursive: true });
  mkdirSync(taskDir);
  
  writeFileSync(join(taskDir, 'task.md'), `---
id: ${id}
name: "Test Task"
description: "Do something"
status: pending
created: 2024-01-01
---

Do it.
`);
  writeFileSync(join(taskDir, 'progress.md'), '');
  return taskDir;
}

after(() => {
  if (existsSync(testDir)) rmSync(testDir, { recursive: true, force: true });
});

test('Worker runs task successfully', async () => {
  const taskDir = setupTask('t_001');

  const provider = new MockProvider([
    { 
      role: 'assistant', 
      content: [{ type: 'text', text: 'Task completed successfully' }] 
    }
  ]);

  const context: ToolContext = {
    cwd: testDir,
    tasksRoot: testDir
  };

  const result = await runWorker(taskDir, [], provider, context);
  
  assert.strictEqual(result.status, 'completed');
  assert.strictEqual(result.result, 'Task completed successfully');
  
  // Check files
  const taskContent = readFileSync(join(taskDir, 'task.md'), 'utf-8');
  const { frontmatter } = parseFrontmatter(taskContent);
  assert.strictEqual(frontmatter.status, 'completed');
  
  const resultContent = readFileSync(join(taskDir, 'result.md'), 'utf-8');
  assert.strictEqual(resultContent, 'Task completed successfully');
});

test('Worker handles error', async () => {
  const taskDir = setupTask('t_002');

  const provider = new MockProvider([]);
  // Override complete to throw
  provider.complete = async () => { throw new Error('Simulated failure'); };

  const context: ToolContext = {
    cwd: testDir,
    tasksRoot: testDir
  };

  const result = await runWorker(taskDir, [], provider, context);
  
  assert.strictEqual(result.status, 'failed');
  assert.strictEqual(result.error, 'Simulated failure');
  
  const taskContent = readFileSync(join(taskDir, 'task.md'), 'utf-8');
  const { frontmatter } = parseFrontmatter(taskContent);
  assert.strictEqual(frontmatter.status, 'failed');

  const resultContent = readFileSync(join(taskDir, 'result.md'), 'utf-8');
  assert.ok(resultContent.includes('Error: Simulated failure'));
});
