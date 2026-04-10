import { test, expect } from 'bun:test';
import { clampScrollOffset, getHistoryWindow, preserveScrollOffsetOnAppend } from '../../../src/clients/tui/history.ts';

test('getHistoryWindow shows the most recent rows at the bottom by default', () => {
  expect(getHistoryWindow([2, 2, 2, 2, 2], 4, 0)).toEqual({
    start: 3,
    end: 5,
    maxScrollOffset: 6,
    scrollOffset: 0,
  });
});

test('getHistoryWindow clamps scrolling to the oldest available rows', () => {
  expect(getHistoryWindow([2, 2, 2, 2, 2], 4, 99)).toEqual({
    start: 0,
    end: 2,
    maxScrollOffset: 6,
    scrollOffset: 6,
  });
});

test('preserveScrollOffsetOnAppend keeps the same viewport anchored when older history is open', () => {
  expect(preserveScrollOffsetOnAppend([2, 2, 2], [2, 2, 2, 3], 4, 2)).toBe(5);
});

test('preserveScrollOffsetOnAppend keeps the same viewport anchored when the last entry grows', () => {
  expect(preserveScrollOffsetOnAppend([2, 2, 2], [2, 2, 4], 4, 2)).toBe(4);
});

test('clampScrollOffset keeps scroll offset in range', () => {
  expect(clampScrollOffset([2, 2], 10, 8)).toBe(0);
  expect(clampScrollOffset([2, 2, 2, 2], 5, -1)).toBe(0);
  expect(clampScrollOffset([2, 2, 2, 2], 3, 20)).toBe(5);
});
