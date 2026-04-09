import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clampScrollOffset, getHistoryWindow, preserveScrollOffsetOnAppend } from '../../../src/clients/tui/history.js';

test('getHistoryWindow shows the most recent rows at the bottom by default', () => {
  assert.deepEqual(getHistoryWindow([2, 2, 2, 2, 2], 4, 0), {
    start: 3,
    end: 5,
    maxScrollOffset: 6,
    scrollOffset: 0,
  });
});

test('getHistoryWindow clamps scrolling to the oldest available rows', () => {
  assert.deepEqual(getHistoryWindow([2, 2, 2, 2, 2], 4, 99), {
    start: 0,
    end: 2,
    maxScrollOffset: 6,
    scrollOffset: 6,
  });
});

test('preserveScrollOffsetOnAppend keeps the same viewport anchored when older history is open', () => {
  assert.equal(preserveScrollOffsetOnAppend([2, 2, 2], [2, 2, 2, 3], 4, 2), 5);
});

test('preserveScrollOffsetOnAppend keeps the same viewport anchored when the last entry grows', () => {
  assert.equal(preserveScrollOffsetOnAppend([2, 2, 2], [2, 2, 4], 4, 2), 4);
});

test('clampScrollOffset keeps scroll offset in range', () => {
  assert.equal(clampScrollOffset([2, 2], 10, 8), 0);
  assert.equal(clampScrollOffset([2, 2, 2, 2], 5, -1), 0);
  assert.equal(clampScrollOffset([2, 2, 2, 2], 3, 20), 5);
});
