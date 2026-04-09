import { resolve } from 'node:path';
import { ToolRegistry } from '../core/tool-registry.js';
import { loadConfig, PicoConfig } from '../config/config.js';
import { createProvider } from '../providers/index.js';
import { listFilesTool } from '../tools/list-files.js';
import { readFileTool } from '../tools/read-file.js';
import { runCommandTool } from '../tools/run-command.js';
import { searchTextTool } from '../tools/search-text.js';
import { writeFileTool } from '../tools/write-file.js';

export interface AppBootstrap {
  config: PicoConfig;
  controlDir: string;
  provider: ReturnType<typeof createProvider>;
  registry: ToolRegistry;
}

export function createAppBootstrap(controlDir = process.cwd()): AppBootstrap {
  const resolvedControlDir = resolve(controlDir);
  const config = loadConfig(resolvedControlDir);
  const provider = createProvider(config);
  const registry = new ToolRegistry({
    tools: [listFilesTool, readFileTool, searchTextTool, writeFileTool, runCommandTool],
    agentTools: {
      ask: ['list_files', 'read_file', 'search_text'],
      exec: ['list_files', 'read_file', 'search_text', 'write_file', 'run_command'],
    },
  });

  return {
    config,
    controlDir: resolvedControlDir,
    provider,
    registry,
  };
}
