import { resolve } from 'node:path';
import { ToolRegistry } from '../core/tool-registry.js';
import { cmdTool } from '../tools/cmd.js';
import { globTool } from '../tools/glob.js';
import { grepTool } from '../tools/grep.js';
import { patchTool } from '../tools/patch.js';
import { readTool } from '../tools/read.js';

export interface RuntimeContext {
  controlDir: string;
  registry: ToolRegistry;
}

export function createRuntimeContext(controlDir = process.cwd()): RuntimeContext {
  const resolvedControlDir = resolve(controlDir);
  const registry = new ToolRegistry({
    tools: [
      globTool,
      grepTool,
      readTool,
      patchTool,
      cmdTool,
    ],
    agentTools: {
      ask: ['glob', 'grep', 'read'],
      exec: [
        'glob',
        'grep',
        'read',
        'patch',
        'cmd',
      ],
    },
  });

  return {
    controlDir: resolvedControlDir,
    registry,
  };
}
