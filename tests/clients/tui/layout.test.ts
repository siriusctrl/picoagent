import { test } from 'node:test';
import assert from 'node:assert/strict';
import { countWrappedLines, estimateEntryHeight } from '../../../src/clients/tui/layout.js';

test('countWrappedLines accounts for wrapping and blank content', () => {
  assert.equal(countWrappedLines('', 10), 1);
  assert.equal(countWrappedLines('abcd', 2), 2);
  assert.equal(countWrappedLines('a\nbcdef', 3), 3);
});

test('estimateEntryHeight does not add a phantom output row for pending tool calls', () => {
  assert.equal(
    estimateEntryHeight({ type: 'tool', title: 'read_file', status: 'pending' }, 40),
    2,
  );
  assert.equal(
    estimateEntryHeight({ type: 'tool', title: 'read_file', status: 'done', output: 'ok' }, 40),
    3,
  );
});
