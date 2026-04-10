import { loadConfigFromContents, PicoConfig, userConfigPath, userHomeDir } from '../config/config.ts';
import type { Filesystem } from '../core/filesystem.ts';
import { ToolRegistry } from '../core/tool-registry.ts';
import { AgentPresetId } from '../core/types.ts';
import { extnamePath, joinPath, relativePath } from '../fs/path.ts';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.ts';
import { DocMeta, scanMarkdownDocuments } from '../prompting/frontmatter.ts';
import { buildSystemPrompt, ControlPromptSurface } from '../prompting/prompt.ts';

const DEFAULTS_DIR = joinPath(import.meta.dir, '..', '..', 'defaults');
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
        key: `${keyPrefix}/${normalizePath(relativePath(root, filePath))}`,
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
    .filter((filePath) => extnamePath(filePath) === '.md')
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

  for (const doc of await scanMarkdownTree(hostFilesystem, joinPath(DEFAULTS_DIR, subdir))) {
    const name = typeof doc.frontmatter.name === 'string' ? doc.frontmatter.name : doc.path;
    merged.set(name, doc);
  }

  for (const doc of await scanMarkdownTree(workspaceFilesystem, joinPath(workspaceRoot, subdir))) {
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
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, 'SOUL.md')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, 'USER.md')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, 'AGENTS.md')),
    readOptional(hostFilesystem, joinPath(userHomeDir(), '.pico', 'memory', 'memory.md')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, '.pico', 'memory', 'memory.md')),
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
    loadControlFile(workspaceFilesystem, 'workspace/SOUL.md', joinPath(workspaceRoot, 'SOUL.md')),
    loadControlFile(workspaceFilesystem, 'workspace/USER.md', joinPath(workspaceRoot, 'USER.md')),
    loadControlFile(workspaceFilesystem, 'workspace/AGENTS.md', joinPath(workspaceRoot, 'AGENTS.md')),
    loadControlFile(workspaceFilesystem, 'workspace/.pico/config.jsonc', joinPath(workspaceRoot, '.pico', 'config.jsonc')),
    loadControlFile(workspaceFilesystem, 'workspace/.pico/memory/memory.md', joinPath(workspaceRoot, '.pico', 'memory', 'memory.md')),
    loadControlFile(hostFilesystem, 'user/.pico/config.jsonc', userConfigPath()),
    loadControlFile(hostFilesystem, 'user/.pico/memory/memory.md', joinPath(userHomeDir(), '.pico', 'memory', 'memory.md')),
  ]);

  const trees = await Promise.all([
    loadControlTree(hostFilesystem, 'defaults/skills', joinPath(DEFAULTS_DIR, 'skills')),
    loadControlTree(hostFilesystem, 'defaults/agents', joinPath(DEFAULTS_DIR, 'agents')),
    loadControlTree(workspaceFilesystem, 'workspace/skills', joinPath(workspaceRoot, 'skills')),
    loadControlTree(workspaceFilesystem, 'workspace/agents', joinPath(workspaceRoot, 'agents')),
  ]);

  return [...files.filter((file): file is LoadedControlFile => file !== null), ...trees.flat()]
    .sort((left, right) => left.key.localeCompare(right.key));
}

export async function computeControlVersion(
  workspaceRoot: string,
  filesystem: Filesystem = new LocalWorkspaceFileSystem(),
): Promise<string> {
  const hash = new Bun.CryptoHasher('sha256');

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
    readOptional(filesystem, joinPath(workspaceRoot, '.pico', 'config.jsonc')),
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
