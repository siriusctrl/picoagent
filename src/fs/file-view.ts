import type { SearchMatch } from '../core/environment.js';

export interface TextBlob {
  path: string;
  content: string;
}

function normalizePath(value: string): string {
  return value.replace(/\\/g, '/').replace(/^\.\/+/, '');
}

function escapeRegExp(value: string): string {
  return value.replace(/[|\\{}()[\]^$+?.]/g, '\\$&');
}

export function globToRegExp(pattern: string): RegExp {
  const normalized = normalizePath(pattern);
  let source = '^';

  for (let index = 0; index < normalized.length; index += 1) {
    const char = normalized[index];

    if (char === '*') {
      const next = normalized[index + 1];
      const afterNext = normalized[index + 2];

      if (next === '*') {
        if (afterNext === '/') {
          source += '(?:.*\\/)?';
          index += 2;
        } else {
          source += '.*';
          index += 1;
        }
        continue;
      }

      source += '[^/]*';
      continue;
    }

    if (char === '?') {
      source += '[^/]';
      continue;
    }

    source += escapeRegExp(char);
  }

  source += '$';
  return new RegExp(source);
}

export function filterGlob(paths: string[], pattern: string, limit = paths.length): string[] {
  const regex = globToRegExp(pattern);
  return paths
    .map((value) => normalizePath(value))
    .filter((value) => regex.test(value))
    .sort((left, right) => left.localeCompare(right))
    .slice(0, limit);
}

export function grepTextBlobs(blobs: TextBlob[], query: string, limit: number, context = 0): SearchMatch[] {
  const needle = query.toLowerCase();
  const matches: SearchMatch[] = [];

  for (const blob of blobs) {
    if (matches.length >= limit) {
      break;
    }

    const lines = blob.content.split(/\r?\n/);
    const selected = new Map<number, SearchMatch['kind']>();

    for (let index = 0; index < lines.length; index += 1) {
      if (!lines[index].toLowerCase().includes(needle)) {
        continue;
      }

      const start = Math.max(index - context, 0);
      const end = Math.min(index + context, lines.length - 1);
      for (let candidate = start; candidate <= end; candidate += 1) {
        selected.set(candidate, selected.get(candidate) === 'match' || candidate === index ? 'match' : 'context');
      }
    }

    for (const [index, kind] of selected) {
      if (matches.length >= limit) {
        break;
      }

      matches.push({
        path: normalizePath(blob.path),
        line: index + 1,
        text: lines[index],
        kind,
      });
    }
  }

  return matches;
}
