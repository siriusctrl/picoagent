import { afterEach, expect, test } from 'bun:test';
import type { Filesystem, ReadTextFileOptions, SearchMatch } from '../../src/core/filesystem.ts';
import { joinPath } from '../../src/fs/path.ts';
import { createRuntimeContext } from '../../src/runtime/index.ts';
import { buildSessionControlSnapshot } from '../../src/runtime/control-snapshot.ts';
import { ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

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

afterEach(async () => {
  for (const dir of tempDirs) {
    await removeDir(dir);
  }
  tempDirs.clear();
});

test('control snapshots keep host defaults and user memory when a workspace filesystem is injected', async () => {
  const workspaceRoot = await makeTempDir('picoagent-control-workspace-');
  const homeRoot = await makeTempDir('picoagent-control-home-');
  tempDirs.add(workspaceRoot);
  tempDirs.add(homeRoot);

  const originalHome = process.env.HOME;
  process.env.HOME = homeRoot;

  try {
    await ensureDir(joinPath(homeRoot, '.pico', 'memory'));
    await writeTextFile(joinPath(homeRoot, '.pico', 'memory', 'memory.md'), 'Remember host preferences.');
    await writeTextFile(joinPath(homeRoot, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "host-echo" }\n');

    const filesystem = new WorkspaceOnlyFilesystem(new Map([
      [joinPath(workspaceRoot, 'AGENTS.md'), 'Workspace agent instructions.'],
    ]));

    const snapshot = await buildSessionControlSnapshot(
      workspaceRoot,
      createRuntimeContext(workspaceRoot).registry,
      filesystem,
    );

    expect(snapshot.config.model).toBe('host-echo');
    expect(snapshot.systemPrompts.ask).toMatch(/Remember host preferences\./);
    expect(snapshot.systemPrompts.ask).toMatch(/reviewer: Code review/);
    expect(snapshot.systemPrompts.ask).toMatch(/Workspace agent instructions\./);
  } finally {
    process.env.HOME = originalHome;
  }
});
