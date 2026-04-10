import type { ExecutionRequest, ExecutionResult } from './execution.js';
import type { SearchMatch } from './filesystem.js';

export type FileViewTarget = 'workspace' | 'session';

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
  glob(path: NamespaceLikePath, pattern: string, limit?: number): Promise<string[]>;
  glob(target: FileViewTarget, pattern: string, limit?: number): Promise<string[]>;
  grep(path: NamespaceLikePath, query: string, options?: { path?: string; limit?: number; context?: number }): Promise<SearchMatch[]>;
  grep(target: FileViewTarget, query: string, options?: { path?: string; limit?: number; context?: number }): Promise<SearchMatch[]>;
  read(path: NamespaceLikePath, options?: FileViewReadOptions): Promise<string>;
  read(target: FileViewTarget, path: string, options?: FileViewReadOptions): Promise<string>;
  patch(operations: FilePatchOperation[]): Promise<FilePatchChange[]>;
  patch(target: FileViewTarget, operations: FilePatchOperation[]): Promise<FilePatchChange[]>;
  cmd(path: NamespaceLikePath, request: Omit<ExecutionRequest, 'runId'>): Promise<ExecutionResult>;
  cmd(target: FileViewTarget, request: Omit<ExecutionRequest, 'runId'>): Promise<ExecutionResult>;
}
