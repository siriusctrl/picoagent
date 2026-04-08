import { homedir } from 'os';
import { join, resolve } from 'path';
import { DEFAULT_CONFIG } from '../hooks/compaction.js';
import { loadConfig, PicoConfig } from '../lib/config.js';
import { buildMainPrompt } from '../lib/prompt.js';
import { RunWorkspace, createRunWorkspace } from '../lib/workspace.js';
import { createProvider } from '../providers/index.js';
import { Runtime, RuntimeEventHandlers } from '../runtime/runtime.js';
import { abortTool } from '../tools/abort.js';
import { dispatchTool } from '../tools/dispatch.js';
import { loadTool } from '../tools/load.js';
import { readFileTool } from '../tools/read-file.js';
import { scanTool } from '../tools/scan.js';
import { shellTool } from '../tools/shell.js';
import { steerTool } from '../tools/steer.js';
import { writeFileTool } from '../tools/write-file.js';
import { Tool, ToolContext } from '../core/types.js';

export interface AppBootstrap {
  config: PicoConfig;
  context: ToolContext;
  controlDir: string;
  runtime: Runtime;
  runWorkspace: RunWorkspace;
  systemPrompt: string;
}

function createToolSets(): { mainTools: Tool[]; workerTools: Tool[] } {
  const workerTools = [
    shellTool,
    readFileTool,
    writeFileTool,
    scanTool,
    loadTool,
  ];

  const mainTools = [
    ...workerTools,
    dispatchTool,
    steerTool,
    abortTool,
  ];

  return { mainTools, workerTools };
}

export function createAppBootstrap(
  controlDir = process.cwd(),
  runtimeHandlers?: RuntimeEventHandlers,
): AppBootstrap {
  const resolvedControlDir = resolve(controlDir);
  const config = loadConfig(resolvedControlDir);
  const runWorkspace = createRunWorkspace({ controlDir: resolvedControlDir });
  const systemPrompt = buildMainPrompt(resolvedControlDir);
  const provider = createProvider(config, systemPrompt);
  const { mainTools, workerTools } = createToolSets();

  const context: ToolContext = {
    controlRoot: resolvedControlDir,
    cwd: runWorkspace.mode === 'attached-git' ? resolvedControlDir : runWorkspace.repoDir,
    repoRoot: runWorkspace.repoDir,
    tasksRoot: runWorkspace.tasksDir,
  };

  const traceDir = join(homedir(), '.picoagent', 'traces');
  const compactionConfig = { ...DEFAULT_CONFIG, contextWindow: config.contextWindow };

  const runtime = new Runtime(
    provider,
    mainTools,
    workerTools,
    context,
    systemPrompt,
    traceDir,
    compactionConfig,
    runtimeHandlers,
  );

  context.onTaskCreated = (taskDir) => runtime.spawnWorker(taskDir);
  context.onSteer = (taskId, message) => runtime.getControl(taskId)?.steer(message);
  context.onAbort = (taskId) => runtime.getControl(taskId)?.abort();

  return {
    config,
    context,
    controlDir: resolvedControlDir,
    runtime,
    runWorkspace,
    systemPrompt,
  };
}
