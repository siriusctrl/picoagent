import { promises as fs } from 'node:fs';
import path from 'node:path';
import type { MutableFilesystem, ReadTextFileOptions, SearchMatch } from '../core/filesystem.js';
import { searchFiles, walkFiles } from './filesystem.js';

export interface WorkspaceFileSystem extends MutableFilesystem {}

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
  constructor(private readonly delegate?: WorkspaceFileSystem) {}

  async readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string> {
    if (this.delegate) {
      return this.delegate.readTextFile(filePath, options);
    }

    return sliceTextByLines(await fs.readFile(filePath, 'utf8'), options);
  }

  async writeTextFile(filePath: string, content: string): Promise<void> {
    if (this.delegate) {
      await this.delegate.writeTextFile(filePath, content);
      return;
    }

    await fs.mkdir(path.dirname(filePath), { recursive: true });
    await fs.writeFile(filePath, content, 'utf8');
  }

  async deleteTextFile(filePath: string): Promise<void> {
    if (this.delegate) {
      await this.delegate.deleteTextFile(filePath);
      return;
    }

    await fs.rm(filePath, { force: true });
  }

  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
    if (this.delegate) {
      return this.delegate.listFiles(root, limit, signal);
    }

    return walkFiles(root, limit, signal);
  }

  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]> {
    if (this.delegate) {
      return this.delegate.searchText(root, query, limit, signal);
    }

    return searchFiles(root, query, limit, signal);
  }
}
