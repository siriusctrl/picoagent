import { Dirent } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import type { SearchMatch } from '../core/filesystem.js';

const SKIP_DIRS = new Set(['.git', 'node_modules', 'dist']);

export function resolveSessionPath(inputPath: string, cwd: string, roots: string[]): string {
  const resolved = path.resolve(cwd, inputPath);
  if (roots.some((root) => isWithinRoot(resolved, root))) {
    return resolved;
  }

  throw new Error(`Path is outside the session roots: ${inputPath}`);
}

export function relativeToCwd(targetPath: string, cwd: string): string {
  const relative = path.relative(cwd, targetPath);
  return relative === '' ? '.' : relative;
}

function isWithinRoot(targetPath: string, root: string): boolean {
  const relative = path.relative(root, targetPath);
  return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

function shouldSkipDir(entry: Dirent): boolean {
  return entry.isDirectory() && SKIP_DIRS.has(entry.name);
}

export async function walkFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
  const results: string[] = [];

  async function visit(dir: string): Promise<void> {
    if (signal.aborted || results.length >= limit) {
      return;
    }

    const entries = (await readdir(dir, { withFileTypes: true })).sort((left, right) =>
      left.name.localeCompare(right.name),
    );

    for (const entry of entries) {
      if (signal.aborted || results.length >= limit) {
        return;
      }

      if (shouldSkipDir(entry)) {
        continue;
      }

      const fullPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        await visit(fullPath);
        continue;
      }

      if (entry.isFile()) {
        results.push(fullPath);
      }
    }
  }

  await visit(root);
  return results;
}

export async function searchFiles(
  root: string,
  query: string,
  limit: number,
  signal: AbortSignal,
): Promise<SearchMatch[]> {
  const matches: SearchMatch[] = [];
  const needle = query.toLowerCase();

  async function visit(dir: string): Promise<void> {
    if (signal.aborted || matches.length >= limit) {
      return;
    }

    const entries = (await readdir(dir, { withFileTypes: true })).sort((left, right) =>
      left.name.localeCompare(right.name),
    );

    for (const entry of entries) {
      if (signal.aborted || matches.length >= limit) {
        return;
      }

      if (shouldSkipDir(entry)) {
        continue;
      }

      const fullPath = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        await visit(fullPath);
        continue;
      }

      if (!entry.isFile()) {
        continue;
      }

      let content: string;
      try {
        content = await readFile(fullPath, 'utf8');
      } catch {
        continue;
      }

      if (content.includes('\u0000')) {
        continue;
      }

      const lines = content.split(/\r?\n/);
      for (let index = 0; index < lines.length; index += 1) {
        if (signal.aborted || matches.length >= limit) {
          return;
        }

        if (!lines[index].toLowerCase().includes(needle)) {
          continue;
        }

        matches.push({
          path: fullPath,
          line: index + 1,
          text: lines[index],
        });
      }
    }
  }

  await visit(root);

  return matches;
}

export function formatLineNumberedText(content: string, line = 1): string {
  const lines = content.split(/\r?\n/);
  return lines
    .map((value, index) => `${String(line + index).padStart(4, ' ')} | ${value}`)
    .join('\n');
}
