import { loadConfigFromContents, PicoConfig, userConfigPath, userHomeDir } from '../config/config.ts';
import type { Filesystem } from '../core/filesystem.ts';
import { ToolRegistry } from '../core/tool-registry.ts';
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
  systemPrompt: string;
}

interface LoadedControlFile {
  key: string;
  path: string;
  content: string;
}

interface LoadedMarkdownTree {
  files: LoadedControlFile[];
  docs: DocMeta[];
}

interface LoadedControlState {
  files: LoadedControlFile[];
  userConfig: string | null;
  workspaceConfig: string | null;
  surface: ControlPromptSurface;
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

function sortLoadedFiles(files: LoadedControlFile[]): LoadedControlFile[] {
  return files.sort((left, right) => left.key.localeCompare(right.key));
}

async function loadMarkdownTree(
  filesystem: Filesystem,
  keyPrefix: string,
  root: string,
): Promise<LoadedMarkdownTree> {
  const filePaths = (await listExistingFiles(filesystem, root))
    .filter((filePath) => extnamePath(filePath) === '.md')
    .sort((left, right) => left.localeCompare(right));

  const files = (await Promise.all(filePaths.map(async (filePath) => {
    try {
      return {
        key: `${keyPrefix}/${normalizePath(relativePath(root, filePath))}`,
        path: filePath,
        content: await filesystem.readTextFile(filePath),
      } satisfies LoadedControlFile;
    } catch {
      return null;
    }
  }))).filter((file): file is LoadedControlFile => file !== null);

  return {
    files: sortLoadedFiles(files),
    docs: scanMarkdownDocuments(files.map(({ path, content }) => ({ path, content }))),
  };
}

function mergeDocuments(...trees: DocMeta[][]): DocMeta[] {
  const merged = new Map<string, DocMeta>();

  for (const tree of trees) {
    for (const doc of tree) {
      const name = typeof doc.frontmatter.name === 'string' ? doc.frontmatter.name : doc.path;
      merged.set(name, doc);
    }
  }

  return [...merged.values()];
}

async function loadControlState(
  workspaceRoot: string,
  workspaceFilesystem: Filesystem,
  hostFilesystem: Filesystem = HOST_FILESYSTEM,
): Promise<LoadedControlState> {
  const [
    soul,
    user,
    agents,
    workspaceConfig,
    workspaceMemory,
    userConfig,
    userMemory,
    defaultSkills,
    workspaceSkills,
  ] = await Promise.all([
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, 'SOUL.md')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, 'USER.md')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, 'AGENTS.md')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, '.pico', 'config.jsonc')),
    readOptional(workspaceFilesystem, joinPath(workspaceRoot, '.pico', 'memory', 'memory.md')),
    readOptional(hostFilesystem, userConfigPath()),
    readOptional(hostFilesystem, joinPath(userHomeDir(), '.pico', 'memory', 'memory.md')),
    loadMarkdownTree(hostFilesystem, 'defaults/skills', joinPath(DEFAULTS_DIR, 'skills')),
    loadMarkdownTree(workspaceFilesystem, 'workspace/skills', joinPath(workspaceRoot, 'skills')),
  ]);

  const files = sortLoadedFiles([
    ...(soul === null ? [] : [{ key: 'workspace/SOUL.md', path: joinPath(workspaceRoot, 'SOUL.md'), content: soul }]),
    ...(user === null ? [] : [{ key: 'workspace/USER.md', path: joinPath(workspaceRoot, 'USER.md'), content: user }]),
    ...(agents === null ? [] : [{ key: 'workspace/AGENTS.md', path: joinPath(workspaceRoot, 'AGENTS.md'), content: agents }]),
    ...(workspaceConfig === null
      ? []
      : [{ key: 'workspace/.pico/config.jsonc', path: joinPath(workspaceRoot, '.pico', 'config.jsonc'), content: workspaceConfig }]),
    ...(workspaceMemory === null
      ? []
      : [{ key: 'workspace/.pico/memory/memory.md', path: joinPath(workspaceRoot, '.pico', 'memory', 'memory.md'), content: workspaceMemory }]),
    ...(userConfig === null
      ? []
      : [{ key: 'user/.pico/config.jsonc', path: userConfigPath(), content: userConfig }]),
    ...(userMemory === null
      ? []
      : [{ key: 'user/.pico/memory/memory.md', path: joinPath(userHomeDir(), '.pico', 'memory', 'memory.md'), content: userMemory }]),
    ...defaultSkills.files,
    ...workspaceSkills.files,
  ]);

  return {
    files,
    userConfig,
    workspaceConfig,
    surface: {
      soul,
      user,
      agents,
      memories: [userMemory, workspaceMemory].filter((value): value is string => value !== null),
      skills: mergeDocuments(defaultSkills.docs, workspaceSkills.docs),
    },
  };
}

function hashControlFiles(files: LoadedControlFile[]): string {
  const hash = new Bun.CryptoHasher('sha256');

  for (const file of files) {
    hash.update(file.key);
    hash.update('\0');
    hash.update(file.content);
    hash.update('\0');
  }

  return hash.digest('hex');
}

export async function computeControlVersion(
  workspaceRoot: string,
  filesystem: Filesystem = new LocalWorkspaceFileSystem(),
): Promise<string> {
  const state = await loadControlState(workspaceRoot, filesystem, HOST_FILESYSTEM);
  return hashControlFiles(state.files);
}

export async function buildSessionControlSnapshot(
  workspaceRoot: string,
  registry: ToolRegistry,
  filesystem: Filesystem = new LocalWorkspaceFileSystem(),
  controlVersion?: string,
): Promise<SessionControlSnapshot> {
  const state = await loadControlState(workspaceRoot, filesystem, HOST_FILESYSTEM);

  return {
    workspaceRoot,
    controlVersion: controlVersion ?? hashControlFiles(state.files),
    config: loadConfigFromContents(workspaceRoot, {
      userConfig: state.userConfig,
      workspaceConfig: state.workspaceConfig,
    }),
    systemPrompt: buildSystemPrompt(state.surface, registry.all()),
  };
}
