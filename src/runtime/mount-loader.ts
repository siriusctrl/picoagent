import type { NamespaceMount } from '../fs/namespace.ts';
import { resolvePath } from '../fs/path.ts';
import { HttpFilesystem } from '../fs/http-filesystem.ts';
import { RootedFilesystem } from '../fs/rooted-fs.ts';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.ts';

export interface RuntimeMountSpec {
  label: string;
  source: string;
}

const RESERVED_MOUNT_NAMES = new Set(['workspace', 'session']);

function normalizeLabel(label: string): string {
  return label.replace(/^\/+|\/+$/g, '');
}

function resolveLocalMountSource(source: string, cwd: string): string {
  const localSource = source.startsWith('local:') ? source.slice('local:'.length) : source;
  return resolvePath(cwd, localSource || '.');
}

async function validateLocalRoot(root: string): Promise<void> {
  if (await Bun.file(root).exists()) {
    throw new Error(`Local mount root must be a directory: ${root}`);
  }

  try {
    for await (const _entry of new Bun.Glob('*').scan({
      cwd: root,
      onlyFiles: false,
      dot: true,
      followSymlinks: false,
    })) {
      break;
    }
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes('ENOENT')) {
      throw new Error(`Local mount root does not exist: ${root}`);
    }

    if (message.includes('ENOTDIR')) {
      throw new Error(`Local mount root must be a directory: ${root}`);
    }

    throw error;
  }
}

function isHttpSource(source: string): boolean {
  return source.startsWith('http://') || source.startsWith('https://');
}

export async function loadRuntimeMounts(mounts: RuntimeMountSpec[], cwd: string): Promise<NamespaceMount[]> {
  const seen = new Set<string>();
  const resolved: NamespaceMount[] = [];

  for (const mount of mounts) {
    const label = normalizeLabel(mount.label);
    if (!label) {
      throw new Error('Mount label must not be empty');
    }

    if (RESERVED_MOUNT_NAMES.has(label)) {
      throw new Error(`Mount label is reserved: ${label}`);
    }

    if (seen.has(label)) {
      throw new Error(`Duplicate mount label: ${label}`);
    }

    seen.add(label);

    if (isHttpSource(mount.source)) {
      const filesystem = new HttpFilesystem(mount.source.replace(/\/+$/, ''));
      const info = await filesystem.getInfo();
      resolved.push({
        name: label,
        filesystem,
        root: '.',
        writable: info.writable,
        supportsCmd: false,
      });
      continue;
    }

    const root = resolveLocalMountSource(mount.source, cwd);
    await validateLocalRoot(root);
    resolved.push({
      name: label,
      filesystem: new RootedFilesystem(new LocalWorkspaceFileSystem(), root),
      root: '.',
      writable: true,
      supportsCmd: false,
    });
  }

  return resolved;
}
