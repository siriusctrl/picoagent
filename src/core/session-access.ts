export interface SessionCompactResult {
  checkpointId: string;
  summary: string;
  compactedMessages: number;
  keptMessages: number;
}

export interface SessionAccess {
  listResources(sessionId: string, path?: string): Promise<string[]>;
  readResource(sessionId: string, path: string): Promise<string>;
  compactSession(sessionId: string, keepLastMessages?: number): Promise<SessionCompactResult>;
}
