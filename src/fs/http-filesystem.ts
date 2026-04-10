import type { MutableFilesystem, ReadTextFileOptions, SearchMatch } from '../core/filesystem.ts';
import type { FilespaceInfo } from '../http/filespace-server.ts';

async function parseJsonResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let message = `Request failed with ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: unknown };
      if (typeof payload.error === 'string') {
        message = payload.error;
      }
    } catch {
      // Ignore malformed error payloads.
    }

    throw new Error(message);
  }

  return response.json() as Promise<T>;
}

export class HttpFilesystem implements MutableFilesystem {
  constructor(private readonly baseUrl: string) {}

  async getInfo(): Promise<FilespaceInfo> {
    const response = await fetch(`${this.baseUrl}/info`);
    return parseJsonResponse<FilespaceInfo>(response);
  }

  async readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string> {
    const response = await fetch(`${this.baseUrl}/read`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ path: filePath, options }),
    });
    const payload = await parseJsonResponse<{ content: string }>(response);
    return payload.content;
  }

  async writeTextFile(filePath: string, content: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}/write`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ path: filePath, content }),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async deleteTextFile(filePath: string): Promise<void> {
    const response = await fetch(`${this.baseUrl}/delete`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ path: filePath }),
    });
    await parseJsonResponse<{ ok: true }>(response);
  }

  async listFiles(root: string, limit: number, _signal: AbortSignal): Promise<string[]> {
    const response = await fetch(`${this.baseUrl}/list`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ root, limit }),
    });
    const payload = await parseJsonResponse<{ paths: string[] }>(response);
    return payload.paths;
  }

  async searchText(root: string, query: string, limit: number, _signal: AbortSignal): Promise<SearchMatch[]> {
    const response = await fetch(`${this.baseUrl}/search`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ root, query, limit }),
    });
    const payload = await parseJsonResponse<{ matches: SearchMatch[] }>(response);
    return payload.matches;
  }
}
