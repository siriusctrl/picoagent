import path from 'node:path';
import { statSync } from 'node:fs';
import type { NamespaceMount } from '../fs/namespace.js';
import { HttpFilesystem } from '../fs/http-filesystem.js';
import { RootedFilesystem } from '../fs/rooted-fs.js';
import { LocalWorkspaceFileSystem } from '../fs/workspace-fs.js';

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
  return path.resolve(cwd, localSource || '.');
}

function validateLocalRoot(root: string): void {
  const stats = statSync(root, { throwIfNoEntry: false });
  if (!stats) {
    throw new Error(`Local mount root does not exist: ${root}`);
  }

  if (!stats.isDirectory()) {
    throw new Error(`Local mount root must be a directory: ${root}`);
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
        executable: false,
      });
      continue;
    }

    const root = resolveLocalMountSource(mount.source, cwd);
    validateLocalRoot(root);
    resolved.push({
      name: label,
      filesystem: new RootedFilesystem(new LocalWorkspaceFileSystem(), root),
      root: '.',
      writable: true,
      executable: false,
    });
  }

  return resolved;
}
