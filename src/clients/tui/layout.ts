export type LayoutEntry =
  | { type: 'system' | 'error'; text: string }
  | { type: 'user' | 'assistant'; text: string }
  | { type: 'tool'; title: string; status: string; output?: string };

export function countWrappedLines(text: string, width: number): number {
  if (!text) {
    return 1;
  }

  return text.split('\n').reduce((total, line) => total + Math.max(Math.ceil(Math.max(line.length, 1) / width), 1), 0);
}

export function estimateEntryHeight(entry: LayoutEntry, contentWidth: number): number {
  if (entry.type === 'tool') {
    const outputHeight = entry.output ? countWrappedLines(entry.output, contentWidth) : 0;
    return countWrappedLines(`${entry.title} [${entry.status}]`, contentWidth) + outputHeight + 1;
  }

  return countWrappedLines(entry.text, contentWidth) + 1;
}
