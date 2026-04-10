import type { SearchMatch } from '../core/filesystem.ts';
import { isAbsolutePath, normalizePath, relativePath, resolvePath } from './path.ts';

const SKIP_DIRS = new Set(['.git', 'node_modules', 'dist']);
const LOCAL_FILE_GLOB = new Bun.Glob('**/*');

export function resolveSessionPath(inputPath: string, cwd: string, roots: string[]): string {
  const resolved = resolvePath(cwd, inputPath);
  if (roots.some((root) => isWithinRoot(resolved, root))) {
    return resolved;
  }

  throw new Error(`Path is outside the session roots: ${inputPath}`);
}

export function relativeToCwd(targetPath: string, cwd: string): string {
  const relative = relativePath(cwd, targetPath);
  return relative === '' ? '.' : relative;
}

function isWithinRoot(targetPath: string, root: string): boolean {
  const relative = relativePath(root, targetPath);
  return relative === '' || (!relative.startsWith('..') && !isAbsolutePath(relative));
}

function shouldSkipPath(root: string, filePath: string): boolean {
  return relativePath(root, filePath).split('/').some((segment) => SKIP_DIRS.has(segment));
}

async function scanLocalFiles(root: string, signal: AbortSignal): Promise<string[]> {
  const results: string[] = [];

  for await (const filePath of LOCAL_FILE_GLOB.scan({
    cwd: root,
    absolute: true,
    dot: true,
    onlyFiles: true,
    followSymlinks: false,
  })) {
    if (signal.aborted) {
      break;
    }

    if (shouldSkipPath(root, filePath)) {
      continue;
    }

    results.push(normalizePath(filePath));
  }

  return results.sort((left, right) => left.localeCompare(right));
}

export async function walkFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
  return (await scanLocalFiles(root, signal)).slice(0, limit);
}

export async function searchFiles(
  root: string,
  query: string,
  limit: number,
  signal: AbortSignal,
): Promise<SearchMatch[]> {
  const matches: SearchMatch[] = [];
  const needle = query.toLowerCase();
  const decoder = new TextDecoder();
  const files = await scanLocalFiles(root, signal);

  for (const filePath of files) {
    if (signal.aborted || matches.length >= limit) {
      break;
    }

    let bytes: Uint8Array;
    try {
      bytes = await Bun.file(filePath).bytes();
    } catch {
      continue;
    }

    if (bytes.includes(0)) {
      continue;
    }

    const lines = decoder.decode(bytes).split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      if (signal.aborted || matches.length >= limit) {
        break;
      }

      if (!lines[index].toLowerCase().includes(needle)) {
        continue;
      }

      matches.push({
        path: filePath,
        line: index + 1,
        text: lines[index],
      });
    }
  }

  return matches;
}

export function formatLineNumberedText(content: string, line = 1): string {
  const lines = content.split(/\r?\n/);
  return lines
    .map((value, index) => `${String(line + index).padStart(4, ' ')} | ${value}`)
    .join('\n');
}
