import type { Filesystem, ReadTextFileOptions, SearchMatch } from '../core/filesystem.ts';
import type { SessionStore } from './store.ts';

function normalizeSessionPath(value: string): string {
  return value.replace(/^\/+|\/+$/g, '') || '.';
}

async function listSessionHistoryPaths(store: SessionStore, sessionId: string): Promise<string[]> {
  const resources = (await store.listSessionResources(sessionId, 'checkpoints')) ?? [];
  const runs = (await store.listSessionResources(sessionId, 'runs')) ?? [];

  return [
    'summary.md',
    ...resources.map((name) => `checkpoints/${name}`),
    ...runs.map((name) => `runs/${name}`),
  ];
}

function sliceTextByLines(content: string, options?: ReadTextFileOptions): string {
  if (!options?.line && !options?.limit) {
    return content;
  }

  const lines = content.split(/\r?\n/);
  const start = Math.max((options.line ?? 1) - 1, 0);
  const end = options.limit ? start + options.limit : undefined;
  return lines.slice(start, end).join('\n');
}

export class SessionFilesystem implements Filesystem {
  constructor(
    private readonly store: SessionStore,
    private readonly sessionId: string,
  ) {}

  async readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string> {
    const normalized = normalizeSessionPath(filePath);
    if (!(await listSessionHistoryPaths(this.store, this.sessionId)).includes(normalized)) {
      throw new Error(`Session file-view path not found: ${filePath}`);
    }

    const content = await this.store.readSessionResource(this.sessionId, normalized);
    if (content === undefined) {
      throw new Error(`Session file-view path not found: ${filePath}`);
    }

    return sliceTextByLines(content, options);
  }

  async listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
    if (signal.aborted) {
      return [];
    }

    const normalizedRoot = normalizeSessionPath(root);
    const allPaths = await listSessionHistoryPaths(this.store, this.sessionId);
    const selected = normalizedRoot === '.'
      ? allPaths
      : allPaths.filter((candidate) => candidate === normalizedRoot || candidate.startsWith(`${normalizedRoot}/`));

    return selected.slice(0, limit);
  }

  async searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]> {
    const needle = query.toLowerCase();
    const matches: SearchMatch[] = [];

    for (const filePath of await this.listFiles(root, Number.MAX_SAFE_INTEGER, signal)) {
      if (signal.aborted || matches.length >= limit) {
        break;
      }

      const content = await this.readTextFile(filePath);
      const lines = content.split(/\r?\n/);
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
}
