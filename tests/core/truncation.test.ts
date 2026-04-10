import { test, expect } from 'bun:test';
import { truncateOutput } from '../../src/core/loop.ts';

test('truncateOutput keeps short content unchanged', () => {
  const content = 'hello world';
  expect(truncateOutput(content)).toBe(content);
});

test('truncateOutput truncates content > 32K', () => {
  const content = 'a'.repeat(33000);
  const result = truncateOutput(content);

  const expectedHead = 'a'.repeat(24000);
  const expectedTail = 'a'.repeat(6000);

  expect(result.slice(0, expectedHead.length)).toBe(expectedHead);
  expect(result.slice(-expectedTail.length)).toBe(expectedTail);
  expect(result).toContain('... [3000 chars truncated] ...');
});
