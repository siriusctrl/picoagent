import { test, expect } from 'bun:test';
import { countWrappedLines, estimateEntryHeight } from '../../../src/clients/tui/layout.ts';

test('countWrappedLines accounts for wrapping and blank content', () => {
  expect(countWrappedLines('', 10)).toBe(1);
  expect(countWrappedLines('abcd', 2)).toBe(2);
  expect(countWrappedLines('a\nbcdef', 3)).toBe(3);
});

test('estimateEntryHeight does not add a phantom output row for pending tool calls', () => {
  expect(
    estimateEntryHeight({ type: 'tool', title: 'read', status: 'pending' }, 40),
  ).toBe(2);
  expect(
    estimateEntryHeight({ type: 'tool', title: 'read', status: 'done', output: 'ok' }, 40),
  ).toBe(3);
});
