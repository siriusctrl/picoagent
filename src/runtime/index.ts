import { ToolRegistry } from '../core/tool-registry.ts';
import { resolvePath } from '../fs/path.ts';
import { cmdTool } from '../tools/cmd.ts';
import { globTool } from '../tools/glob.ts';
import { grepTool } from '../tools/grep.ts';
import { patchTool } from '../tools/patch.ts';
import { readTool } from '../tools/read.ts';

export interface RuntimeContext {
  controlDir: string;
  registry: ToolRegistry;
}

export function createRuntimeContext(controlDir = process.cwd()): RuntimeContext {
  const resolvedControlDir = resolvePath(controlDir);
  const registry = new ToolRegistry({
    tools: [
      globTool,
      grepTool,
      readTool,
      patchTool,
      cmdTool,
    ],
  });

  return {
    controlDir: resolvedControlDir,
    registry,
  };
}
