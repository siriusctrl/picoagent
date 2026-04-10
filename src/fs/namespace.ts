import path from 'node:path';
import type { Filesystem, MutableFilesystem, ReadTextFileOptions, SearchMatch } from '../core/filesystem.js';

export interface NamespaceMount {
  name: string;
  filesystem: Filesystem;
  root: string;
  writable?: boolean;
  executable?: boolean;
}

function normalizeMountName(name: string): string {
  const normalized = name.replace(/^\/+|\/+$/g, '');
  if (!normalized) {
    throw new Error('Namespace mount name must not be empty');
  }

  return normalized;
}

function normalizeVirtualPath(inputPath: string): string {
  const normalized = inputPath.replace(/\\/g, '/').replace(/^\/+/, '');
  return normalized || '.';
}

function normalizeNamespacePath(namespacePath: string): string {
  return namespacePath.replace(/\\/g, '/');
}

function toMountedPath(root: string, relativePath: string): string {
  if (root === '.' || root === '') {
    return relativePath === '.' ? '.' : relativePath;
  }

  return relativePath === '.' ? root : path.join(root, relativePath);
}

function fromMountedPath(root: string, mountedPath: string): string {
  if (root === '.' || root === '') {
    return mountedPath.replace(/\\/g, '/');
  }

  const relative = path.relative(root, mountedPath);
  return relative === '' ? '.' : relative.replace(/\\/g, '/');
}

export class Namespace {
  private readonly mounts = new Map<string, NamespaceMount>();

  constructor(mounts: NamespaceMount[]) {
    for (const mount of mounts) {
      this.mounts.set(normalizeMountName(mount.name), {
        ...mount,
        name: normalizeMountName(mount.name),
      });
    }
  }

  resolveNamespacePath(namespacePath: string): { mountName: string; mount: NamespaceMount; relativePath: string; mountedPath: string } {
    const normalized = normalizeNamespacePath(namespacePath);
    if (!normalized.startsWith('/')) {
      throw new Error(`Namespace path must be absolute: ${namespacePath}`);
    }

    const trimmed = normalized.replace(/^\/+/, '');
    const splitIndex = trimmed.indexOf('/');
    const mountName = normalizeMountName(splitIndex === -1 ? trimmed : trimmed.slice(0, splitIndex));
    const relativePath = splitIndex === -1 ? '.' : normalizeVirtualPath(trimmed.slice(splitIndex + 1));
    const mountedPath = this.resolvePath(mountName, relativePath);

    return {
      mountName,
      mount: this.mount(mountName),
      relativePath,
      mountedPath,
    };
  }

  toNamespacePath(name: string, mountedPath: string): string {
    const mount = this.mount(name);
    const relativePath = fromMountedPath(mount.root, mountedPath);

    return relativePath === '.' ? `/${mount.name}` : `/${mount.name}/${relativePath}`;
  }

  mount(name: string): NamespaceMount {
    const mount = this.mounts.get(normalizeMountName(name));
    if (!mount) {
      throw new Error(`Unknown namespace mount: ${name}`);
    }

    return mount;
  }

  writableMount(name: string): NamespaceMount & { filesystem: MutableFilesystem } {
    const mount = this.mount(name);
    if (!mount.writable) {
      throw new Error(`Namespace mount is not writable: ${name}`);
    }

    return mount as NamespaceMount & { filesystem: MutableFilesystem };
  }

  resolvePath(name: string, relativePath: string): string {
    if (path.isAbsolute(relativePath)) {
      return relativePath;
    }

    return toMountedPath(this.mount(name).root, normalizeVirtualPath(relativePath));
  }

  async readTextFile(name: string, relativePath: string, options?: ReadTextFileOptions): Promise<string> {
    const mount = this.mount(name);
    return mount.filesystem.readTextFile(this.resolvePath(name, relativePath), options);
  }

  async writeTextFile(name: string, relativePath: string, content: string): Promise<void> {
    const mount = this.writableMount(name);
    await mount.filesystem.writeTextFile(this.resolvePath(name, relativePath), content);
  }

  async deleteTextFile(name: string, relativePath: string): Promise<void> {
    const mount = this.writableMount(name);
    await mount.filesystem.deleteTextFile(this.resolvePath(name, relativePath));
  }

  async listFiles(name: string, relativeRoot: string, limit: number, signal: AbortSignal): Promise<string[]> {
    const mount = this.mount(name);
    const mountedRoot = this.resolvePath(name, relativeRoot);
    const paths = await mount.filesystem.listFiles(mountedRoot, limit, signal);
    return paths.map((filePath) => fromMountedPath(mount.root, filePath));
  }

  async searchText(
    name: string,
    relativeRoot: string,
    query: string,
    limit: number,
    signal: AbortSignal,
  ): Promise<SearchMatch[]> {
    const mount = this.mount(name);
    const mountedRoot = this.resolvePath(name, relativeRoot);
    const matches = await mount.filesystem.searchText(mountedRoot, query, limit, signal);
    return matches.map((match) => ({
      ...match,
      path: fromMountedPath(mount.root, match.path),
    }));
  }
}
