import { test } from 'node:test';
import assert from 'node:assert';
import { truncateOutput } from '../../src/core/agent-loop.js';

test('truncateOutput keeps short content unchanged', () => {
  const content = 'hello world';
  assert.strictEqual(truncateOutput(content), content);
});

test('truncateOutput truncates content > 32K', () => {
  const content = 'a'.repeat(33000);
  const result = truncateOutput(content);
  
  const expectedHead = 'a'.repeat(24000);
  const expectedTail = 'a'.repeat(6000);
  
  assert.ok(result.startsWith(expectedHead));
  assert.ok(result.endsWith(expectedTail));
  assert.ok(result.includes('... [3000 chars truncated] ...'));
});
