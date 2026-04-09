import type { RunCommandRequest, RunCommandResult, SearchMatch } from './environment.js';

export type FileViewTarget = 'workspace' | 'session';

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
  glob(target: FileViewTarget, pattern: string, limit?: number): Promise<string[]>;
  grep(
    target: FileViewTarget,
    query: string,
    options?: { path?: string; limit?: number; context?: number },
  ): Promise<SearchMatch[]>;
  read(target: FileViewTarget, path: string, options?: FileViewReadOptions): Promise<string>;
  patch(target: FileViewTarget, operations: FilePatchOperation[]): Promise<FilePatchChange[]>;
  cmd(target: FileViewTarget, request: Omit<RunCommandRequest, 'sessionId'>): Promise<RunCommandResult>;
}
