import { createHash } from 'node:crypto';
import { homedir } from 'node:os';
import { dirname, extname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadConfigFromContents, PicoConfig, userConfigPath } from '../config/config.js';
import type { Filesystem } from '../core/filesystem.js';
import { ToolRegistry } from '../core/tool-registry.js';
import { AgentPresetId } from '../core/types.js';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.js';
import { DocMeta, scanMarkdownDocuments } from '../prompting/frontmatter.js';
import { buildSystemPrompt, ControlPromptSurface } from '../prompting/prompt.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const DEFAULTS_DIR = join(__dirname, '..', '..', 'defaults');
const NEVER_ABORTED = new AbortController().signal;
const HOST_FILESYSTEM = new LocalWorkspaceFileSystem();

export interface SessionControlSnapshot {
  workspaceRoot: string;
  controlVersion: string;
  config: PicoConfig;
  systemPrompts: Record<AgentPresetId, string>;
}

interface LoadedControlFile {
  key: string;
  path: string;
  content: string;
}

function normalizePath(value: string): string {
  return value.replace(/\\/g, '/');
}

async function readOptional(filesystem: Filesystem, filePath: string): Promise<string | null> {
  try {
    return (await filesystem.readTextFile(filePath)).trim();
  } catch {
    return null;
  }
}

async function listExistingFiles(filesystem: Filesystem, root: string): Promise<string[]> {
  try {
    return await filesystem.listFiles(root, 2000, NEVER_ABORTED);
  } catch {
    return [];
  }
}

async function loadControlFile(
  filesystem: Filesystem,
  key: string,
  filePath: string,
): Promise<LoadedControlFile | null> {
  const content = await readOptional(filesystem, filePath);
  if (content === null) {
    return null;
  }

  return { key, path: filePath, content };
}

async function loadControlTree(
  filesystem: Filesystem,
  keyPrefix: string,
  root: string,
): Promise<LoadedControlFile[]> {
  const filePaths = await listExistingFiles(filesystem, root);
  const loaded = await Promise.all(filePaths.map(async (filePath) => {
    try {
      return {
        key: `${keyPrefix}/${normalizePath(relative(root, filePath))}`,
        path: filePath,
        content: await filesystem.readTextFile(filePath),
      } satisfies LoadedControlFile;
    } catch {
      return null;
    }
  }));

  return loaded
    .filter((file): file is LoadedControlFile => file !== null)
    .sort((left, right) => left.key.localeCompare(right.key));
}

async function scanMarkdownTree(filesystem: Filesystem, root: string): Promise<DocMeta[]> {
  const filePaths = (await listExistingFiles(filesystem, root))
    .filter((filePath) => extname(filePath) === '.md')
    .sort((left, right) => left.localeCompare(right));

  const documents = await Promise.all(filePaths.map(async (filePath) => {
    try {
      return {
        path: filePath,
        content: await filesystem.readTextFile(filePath),
      };
    } catch {
      return null;
    }
  }));

  return scanMarkdownDocuments(documents.filter((document): document is { path: string; content: string } => document !== null));
}

async function scanMergedDocuments(
  workspaceFilesystem: Filesystem,
  subdir: string,
  workspaceRoot: string,
  hostFilesystem: Filesystem = HOST_FILESYSTEM,
): Promise<DocMeta[]> {
  const merged = new Map<string, DocMeta>();

  for (const doc of await scanMarkdownTree(hostFilesystem, join(DEFAULTS_DIR, subdir))) {
    const name = typeof doc.frontmatter.name === 'string' ? doc.frontmatter.name : doc.path;
    merged.set(name, doc);
  }

  for (const doc of await scanMarkdownTree(workspaceFilesystem, join(workspaceRoot, subdir))) {
    const name = typeof doc.frontmatter.name === 'string' ? doc.frontmatter.name : doc.path;
    merged.set(name, doc);
  }

  return [...merged.values()];
}

async function buildControlPromptSurface(
  workspaceRoot: string,
  workspaceFilesystem: Filesystem,
  hostFilesystem: Filesystem = HOST_FILESYSTEM,
): Promise<ControlPromptSurface> {
  const [soul, user, agents, userMemory, workspaceMemory, skills, agentsDocs] = await Promise.all([
    readOptional(workspaceFilesystem, join(workspaceRoot, 'SOUL.md')),
    readOptional(workspaceFilesystem, join(workspaceRoot, 'USER.md')),
    readOptional(workspaceFilesystem, join(workspaceRoot, 'AGENTS.md')),
    readOptional(hostFilesystem, join(homedir(), '.pico', 'memory', 'memory.md')),
    readOptional(workspaceFilesystem, join(workspaceRoot, '.pico', 'memory', 'memory.md')),
    scanMergedDocuments(workspaceFilesystem, 'skills', workspaceRoot, hostFilesystem),
    scanMergedDocuments(workspaceFilesystem, 'agents', workspaceRoot, hostFilesystem),
  ]);

  return {
    soul,
    user,
    agents,
    memories: [userMemory, workspaceMemory].filter((value): value is string => value !== null),
    skills,
    agentsDocs,
  };
}

async function loadControlFiles(
  workspaceRoot: string,
  workspaceFilesystem: Filesystem,
  hostFilesystem: Filesystem = HOST_FILESYSTEM,
): Promise<LoadedControlFile[]> {
  const files = await Promise.all([
    loadControlFile(workspaceFilesystem, 'workspace/SOUL.md', join(workspaceRoot, 'SOUL.md')),
    loadControlFile(workspaceFilesystem, 'workspace/USER.md', join(workspaceRoot, 'USER.md')),
    loadControlFile(workspaceFilesystem, 'workspace/AGENTS.md', join(workspaceRoot, 'AGENTS.md')),
    loadControlFile(workspaceFilesystem, 'workspace/.pico/config.jsonc', join(workspaceRoot, '.pico', 'config.jsonc')),
    loadControlFile(workspaceFilesystem, 'workspace/.pico/memory/memory.md', join(workspaceRoot, '.pico', 'memory', 'memory.md')),
    loadControlFile(hostFilesystem, 'user/.pico/config.jsonc', userConfigPath()),
    loadControlFile(hostFilesystem, 'user/.pico/memory/memory.md', join(homedir(), '.pico', 'memory', 'memory.md')),
  ]);

  const trees = await Promise.all([
    loadControlTree(hostFilesystem, 'defaults/skills', join(DEFAULTS_DIR, 'skills')),
    loadControlTree(hostFilesystem, 'defaults/agents', join(DEFAULTS_DIR, 'agents')),
    loadControlTree(workspaceFilesystem, 'workspace/skills', join(workspaceRoot, 'skills')),
    loadControlTree(workspaceFilesystem, 'workspace/agents', join(workspaceRoot, 'agents')),
  ]);

  return [...files.filter((file): file is LoadedControlFile => file !== null), ...trees.flat()]
    .sort((left, right) => left.key.localeCompare(right.key));
}

export async function computeControlVersion(
  workspaceRoot: string,
  filesystem: Filesystem = new LocalWorkspaceFileSystem(),
): Promise<string> {
  const hash = createHash('sha256');

  for (const file of await loadControlFiles(workspaceRoot, filesystem, HOST_FILESYSTEM)) {
    hash.update(file.key);
    hash.update('\0');
    hash.update(file.content);
    hash.update('\0');
  }

  return hash.digest('hex');
}

export async function buildSessionControlSnapshot(
  workspaceRoot: string,
  registry: ToolRegistry,
  filesystem: Filesystem = new LocalWorkspaceFileSystem(),
  controlVersion?: string,
): Promise<SessionControlSnapshot> {
  const [resolvedControlVersion, surface, userConfig, workspaceConfig] = await Promise.all([
    controlVersion ?? computeControlVersion(workspaceRoot, filesystem),
    buildControlPromptSurface(workspaceRoot, filesystem, HOST_FILESYSTEM),
    readOptional(HOST_FILESYSTEM, userConfigPath()),
    readOptional(filesystem, join(workspaceRoot, '.pico', 'config.jsonc')),
  ]);

  return {
    workspaceRoot,
    controlVersion: resolvedControlVersion,
    config: loadConfigFromContents(workspaceRoot, {
      userConfig,
      workspaceConfig,
    }),
    systemPrompts: {
      ask: buildSystemPrompt(surface, 'ask', registry.forAgent('ask')),
      exec: buildSystemPrompt(surface, 'exec', registry.forAgent('exec')),
    },
  };
}
