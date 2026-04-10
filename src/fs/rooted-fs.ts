import path from 'node:path';
import type { MutableFilesystem, ReadTextFileOptions, SearchMatch } from '../core/filesystem.js';

function isWithinRoot(targetPath: string, root: string): boolean {
  const relative = path.relative(root, targetPath);
  return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

function normalizeRelativePath(inputPath: string): string {
  return inputPath.replace(/\\/g, '/').replace(/^\/+/, '') || '.';
}

export class RootedFilesystem implements MutableFilesystem {
  constructor(
    private readonly delegate: MutableFilesystem,
    private readonly root: string,
  ) {}

  private resolveRelativePath(inputPath: string): string {
    const resolved = path.resolve(this.root, inputPath);
    if (!isWithinRoot(resolved, this.root)) {
      throw new Error(`Path is outside the rooted filesystem: ${inputPath}`);
    }

    return resolved;
  }

  private toRelativePath(filePath: string): string {
    const relative = path.relative(this.root, filePath);
    return normalizeRelativePath(relative);
  }

  async readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string> {
    return this.delegate.readTextFile(this.resolveRelativePath(filePath), options);
  }

  async writeTextFile(filePath: string, content: string): Promise<void> {
    await this.delegate.writeTextFile(this.resolveRelativePath(filePath), content);
  }

  async deleteTextFile(filePath: string): Promise<void> {
    await this.delegate.deleteTextFile(this.resolveRelativePath(filePath));
  }

  async listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]> {
    const files = await this.delegate.listFiles(this.resolveRelativePath(root), limit, signal);
    return files.map((filePath) => this.toRelativePath(filePath));
  }

  async searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]> {
    const matches = await this.delegate.searchText(this.resolveRelativePath(root), query, limit, signal);
    return matches.map((match) => ({
      ...match,
      path: this.toRelativePath(match.path),
    }));
  }
}
