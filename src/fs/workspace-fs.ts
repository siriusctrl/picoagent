import type { MutableFilesystem, ReadTextFileOptions, SearchMatch } from '../core/filesystem.ts';
import { searchFiles, walkFiles } from './filesystem.ts';

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

    return sliceTextByLines(await Bun.file(filePath).text(), options);
  }

  async writeTextFile(filePath: string, content: string): Promise<void> {
    if (this.delegate) {
      await this.delegate.writeTextFile(filePath, content);
      return;
    }

    await Bun.write(filePath, content);
  }

  async deleteTextFile(filePath: string): Promise<void> {
    if (this.delegate) {
      await this.delegate.deleteTextFile(filePath);
      return;
    }

    const file = Bun.file(filePath);
    if (await file.exists()) {
      await file.delete();
    }
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
