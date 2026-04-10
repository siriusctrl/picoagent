import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, test } from 'node:test';
import type { Filesystem, ReadTextFileOptions, SearchMatch } from '../../src/core/filesystem.js';
import { createRuntimeContext } from '../../src/runtime/index.js';
import { buildSessionControlSnapshot } from '../../src/runtime/control-snapshot.js';

class WorkspaceOnlyFilesystem implements Filesystem {
  constructor(private readonly files = new Map<string, string>()) {}

  async readTextFile(filePath: string, options?: ReadTextFileOptions): Promise<string> {
    const content = this.files.get(filePath);
    if (content === undefined) {
      throw new Error(`Missing file: ${filePath}`);
    }

    if (!options?.line && !options?.limit) {
      return content;
    }

    const lines = content.split(/\r?\n/);
    const start = Math.max((options.line ?? 1) - 1, 0);
    const end = options.limit ? start + options.limit : undefined;
    return lines.slice(start, end).join('\n');
  }

  async listFiles(root: string, limit: number): Promise<string[]> {
    return [...this.files.keys()]
      .filter((filePath) => filePath === root || filePath.startsWith(`${root}/`))
      .sort((left, right) => left.localeCompare(right))
      .slice(0, limit);
  }

  async searchText(root: string, query: string, limit: number): Promise<SearchMatch[]> {
    return [...this.files.entries()]
      .filter(([filePath, content]) => (filePath === root || filePath.startsWith(`${root}/`)) && content.includes(query))
      .slice(0, limit)
      .map(([filePath, content]) => ({
        path: filePath,
        line: 1,
        text: content,
      }));
  }
}

const tempDirs = new Set<string>();

afterEach(() => {
  for (const dir of tempDirs) {
    rmSync(dir, { recursive: true, force: true });
  }
  tempDirs.clear();
});

test('control snapshots keep host defaults and user memory when a workspace filesystem is injected', async () => {
  const workspaceRoot = mkdtempSync(join(tmpdir(), 'picoagent-control-workspace-'));
  const homeRoot = mkdtempSync(join(tmpdir(), 'picoagent-control-home-'));
  tempDirs.add(workspaceRoot);
  tempDirs.add(homeRoot);

  const originalHome = process.env.HOME;
  process.env.HOME = homeRoot;

  try {
    mkdirSync(join(homeRoot, '.pico', 'memory'), { recursive: true });
    writeFileSync(join(homeRoot, '.pico', 'memory', 'memory.md'), 'Remember host preferences.', 'utf8');
    writeFileSync(join(homeRoot, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "host-echo" }\n', 'utf8');

    const filesystem = new WorkspaceOnlyFilesystem(new Map([
      [join(workspaceRoot, 'AGENTS.md'), 'Workspace agent instructions.'],
    ]));

    const snapshot = await buildSessionControlSnapshot(
      workspaceRoot,
      createRuntimeContext(workspaceRoot).registry,
      filesystem,
    );

    assert.equal(snapshot.config.model, 'host-echo');
    assert.match(snapshot.systemPrompts.ask, /Remember host preferences\./);
    assert.match(snapshot.systemPrompts.ask, /reviewer: Code review/);
    assert.match(snapshot.systemPrompts.ask, /Workspace agent instructions\./);
  } finally {
    process.env.HOME = originalHome;
  }
});
