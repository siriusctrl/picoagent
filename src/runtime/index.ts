import { resolve } from 'node:path';
import { ToolRegistry } from '../core/tool-registry.js';
import { listFilesTool } from '../tools/list-files.js';
import { listSessionResourcesTool } from '../tools/list-session-resources.js';
import { readSessionResourceTool } from '../tools/read-session-resource.js';
import { readFileTool } from '../tools/read-file.js';
import { runCommandTool } from '../tools/run-command.js';
import { searchTextTool } from '../tools/search-text.js';
import { compactSessionTool } from '../tools/compact-session.js';
import { writeFileTool } from '../tools/write-file.js';

export interface RuntimeContext {
  controlDir: string;
  registry: ToolRegistry;
}

export function createRuntimeContext(controlDir = process.cwd()): RuntimeContext {
  const resolvedControlDir = resolve(controlDir);
  const registry = new ToolRegistry({
    tools: [
      listFilesTool,
      readFileTool,
      searchTextTool,
      listSessionResourcesTool,
      readSessionResourceTool,
      compactSessionTool,
      writeFileTool,
      runCommandTool,
    ],
    agentTools: {
      ask: ['list_files', 'read_file', 'search_text', 'list_session_resources', 'read_session_resource'],
      exec: [
        'list_files',
        'read_file',
        'search_text',
        'list_session_resources',
        'read_session_resource',
        'compact_session',
        'write_file',
        'run_command',
      ],
    },
  });

  return {
    controlDir: resolvedControlDir,
    registry,
  };
}
