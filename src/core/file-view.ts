import type { ExecutionRequest, ExecutionResult } from './execution.js';
import type { SearchMatch } from './filesystem.js';

export type NamespaceLikePath = `/${string}`;

export interface FileViewReadOptions {
  line?: number;
  limit?: number;
}

export type FilePatchOperation =
  | {
      type: 'create';
      path: string;
      content: string;
    }
  | {
      type: 'replace';
      path: string;
      oldText: string;
      newText: string;
      all?: boolean;
    }
  | {
      type: 'delete';
      path: string;
    };

export interface FilePatchChange {
  path: string;
  action: 'create' | 'update' | 'delete';
  oldText?: string;
  newText?: string;
}

export interface FileViewAccess {
  glob(pattern: NamespaceLikePath, limit?: number): Promise<string[]>;
  grep(query: string, options?: { path?: NamespaceLikePath; limit?: number; context?: number }): Promise<SearchMatch[]>;
  read(path: NamespaceLikePath, options?: FileViewReadOptions): Promise<string>;
  patch(operations: FilePatchOperation[]): Promise<FilePatchChange[]>;
  cmd(request: Omit<ExecutionRequest, 'runId'> & { cwd?: NamespaceLikePath }): Promise<ExecutionResult>;
}
