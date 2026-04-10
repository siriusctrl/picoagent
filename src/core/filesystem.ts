export interface SearchMatch {
  path: string;
  line: number;
  text: string;
  kind?: 'match' | 'context';
}

export interface ReadTextFileOptions {
  line?: number;
  limit?: number;
}

export interface Filesystem {
  readTextFile(path: string, options?: ReadTextFileOptions): Promise<string>;
  listFiles(root: string, limit: number, signal: AbortSignal): Promise<string[]>;
  searchText(root: string, query: string, limit: number, signal: AbortSignal): Promise<SearchMatch[]>;
}

export interface MutableFilesystem extends Filesystem {
  writeTextFile(path: string, content: string): Promise<void>;
  deleteTextFile(path: string): Promise<void>;
}
