import { createHash } from 'node:crypto';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PicoConfig, loadConfig, userConfigPath } from '../config/config.js';
import { ToolRegistry } from '../core/tool-registry.js';
import { AgentPresetId } from '../core/types.js';
import { buildSystemPrompt } from '../prompting/prompt.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const DEFAULTS_DIR = join(__dirname, '..', '..', 'defaults');

export interface SessionControlSnapshot {
  workspaceRoot: string;
  controlVersion: string;
  config: PicoConfig;
  systemPrompts: Record<AgentPresetId, string>;
}

function walkExistingFiles(root: string): string[] {
  if (!existsSync(root)) {
    return [];
  }

  const stat = statSync(root);
  if (stat.isFile()) {
    return [root];
  }

  if (!stat.isDirectory()) {
    return [];
  }

  const files: string[] = [];
  for (const entry of readdirSync(root, { withFileTypes: true }).sort((left, right) => left.name.localeCompare(right.name))) {
    const fullPath = join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...walkExistingFiles(fullPath));
      continue;
    }

    if (entry.isFile()) {
      files.push(fullPath);
    }
  }

  return files;
}

function controlFiles(workspaceRoot: string): string[] {
  return [
    join(workspaceRoot, 'SOUL.md'),
    join(workspaceRoot, 'USER.md'),
    join(workspaceRoot, 'AGENTS.md'),
    join(workspaceRoot, '.pico', 'config.jsonc'),
    join(workspaceRoot, '.pico', 'memory', 'memory.md'),
    userConfigPath(),
    join(homedir(), '.pico', 'memory', 'memory.md'),
    ...walkExistingFiles(join(DEFAULTS_DIR, 'skills')),
    ...walkExistingFiles(join(DEFAULTS_DIR, 'agents')),
    ...walkExistingFiles(join(workspaceRoot, 'skills')),
    ...walkExistingFiles(join(workspaceRoot, 'agents')),
  ].filter((filePath, index, all) => existsSync(filePath) && all.indexOf(filePath) === index);
}

export function computeControlVersion(workspaceRoot: string): string {
  const hash = createHash('sha256');

  for (const filePath of controlFiles(workspaceRoot)) {
    const stat = statSync(filePath);
    hash.update(relative(workspaceRoot, filePath));
    hash.update('\0');
    hash.update(String(stat.size));
    hash.update('\0');
    hash.update(String(stat.mtimeMs));
    hash.update('\0');
  }

  return hash.digest('hex');
}

export function buildSessionControlSnapshot(
  workspaceRoot: string,
  registry: ToolRegistry,
  controlVersion = computeControlVersion(workspaceRoot),
): SessionControlSnapshot {
  return {
    workspaceRoot,
    controlVersion,
    config: loadConfig(workspaceRoot),
    systemPrompts: {
      ask: buildSystemPrompt(workspaceRoot, 'ask', registry.forAgent('ask')),
      exec: buildSystemPrompt(workspaceRoot, 'exec', registry.forAgent('exec')),
    },
  };
}

// TODO: When workspaces stop being local paths, replace this local stat/hash walk with the
// workspace resource's own version contract instead of assuming direct node:fs access.
