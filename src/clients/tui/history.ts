export interface HistoryWindow {
  start: number;
  end: number;
  maxScrollOffset: number;
  scrollOffset: number;
}

function sum(values: number[]): number {
  return values.reduce((total, value) => total + value, 0);
}

function lowerBound(values: number[], target: number): number {
  let low = 0;
  let high = values.length;

  while (low < high) {
    const mid = Math.floor((low + high) / 2);
    if (values[mid] < target) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }

  return low;
}

export function clampScrollOffset(itemHeights: number[], viewportRows: number, scrollOffset: number): number {
  const maxScrollOffset = Math.max(sum(itemHeights) - viewportRows, 0);
  return Math.max(0, Math.min(scrollOffset, maxScrollOffset));
}

export function getHistoryWindow(itemHeights: number[], viewportRows: number, scrollOffset: number): HistoryWindow {
  const prefix = [0];
  for (const height of itemHeights) {
    prefix.push(prefix[prefix.length - 1] + height);
  }

  const totalHeight = prefix[prefix.length - 1];
  const clampedOffset = clampScrollOffset(itemHeights, viewportRows, scrollOffset);
  const endRow = Math.max(totalHeight - clampedOffset, 0);
  const startRow = Math.max(endRow - viewportRows, 0);
  const start = itemHeights.length === 0 ? 0 : lowerBound(prefix.slice(1), startRow + 1);
  const end = lowerBound(prefix, endRow);

  return {
    start,
    end: Math.max(end, start),
    maxScrollOffset: Math.max(totalHeight - viewportRows, 0),
    scrollOffset: clampedOffset,
  };
}

export function preserveScrollOffsetOnAppend(
  previousHeights: number[],
  nextHeights: number[],
  viewportRows: number,
  scrollOffset: number,
): number {
  if (scrollOffset === 0) {
    return clampScrollOffset(nextHeights, viewportRows, scrollOffset);
  }

  const heightDelta = sum(nextHeights) - sum(previousHeights);
  if (heightDelta <= 0) {
    return clampScrollOffset(nextHeights, viewportRows, scrollOffset);
  }

  return clampScrollOffset(nextHeights, viewportRows, scrollOffset + heightDelta);
}
