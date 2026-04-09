import { promises as fs } from 'node:fs';
import path from 'node:path';
import { SearchMatch } from '../core/environment.js';
import { searchFiles, walkFiles } from './filesystem.js';

export interface ReadTextFileOptions {
  line?: number;
  limit?: number;
}

export interface WorkspaceFileSystem {
  readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string>;
  writeTextFile(filePath: string, content: string): Promise<void>;
  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]>;
  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]>;
}

function sliceTextByLines(content: string, options?: ReadTextFileOptions): string {
  if (!options?.line && !options?.limit) {
    return content;
  }

  const lines = content.split(/\r?\n/);
  const start = Math.max((options?.line ?? 1) - 1, 0);
  const end = options?.limit ? start + options.limit : undefined;
  return lines.slice(start, end).join('\n');
}

export class LocalWorkspaceFileSystem implements WorkspaceFileSystem {
  async readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string> {
    return sliceTextByLines(await fs.readFile(filePath, 'utf8'), options);
  }

  async writeTextFile(filePath: string, content: string): Promise<void> {
    await fs.mkdir(path.dirname(filePath), { recursive: true });
    await fs.writeFile(filePath, content, 'utf8');
  }

  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
    return walkFiles(root, limit, signal);
  }

  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]> {
    return searchFiles(root, query, limit, signal);
  }
}

// TODO: Move control-workspace reads for config and prompt docs behind a filesystem boundary too
// if we want a fully virtualized workspace instead of only virtual tool-facing files.
// TODO: `run_command` remains an OS process boundary. Virtual workspaces will need a separate
// command strategy rather than treating command execution as part of the filesystem abstraction.
