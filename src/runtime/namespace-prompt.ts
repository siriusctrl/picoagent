import type { NamespaceMount } from '../fs/namespace.ts';

function describeNamespaceMount(mount: Pick<NamespaceMount, 'name' | 'writable' | 'supportsCmd'>): string {
  if (mount.name === 'workspace') {
    return `- /workspace: main ${mount.writable ? 'read/write' : 'read-only'} workspace, ${mount.supportsCmd ? 'cmd enabled' : 'cmd disabled'}`;
  }

  if (mount.name === 'session') {
    return '- /session: read-only session history, cmd disabled';
  }

  return `- /${mount.name}: mounted ${mount.writable ? 'read/write' : 'read-only'} file-view, ${mount.supportsCmd ? 'cmd enabled' : 'cmd disabled'}`;
}

export function buildNamespacePromptSection(
  mounts: Array<Pick<NamespaceMount, 'name' | 'writable' | 'supportsCmd'>>,
): string {
  return [
    '## File Views',
    ...mounts.map(describeNamespaceMount),
    '- cmd always requires an explicit cwd path like /workspace or /sandbox/tmp.',
  ].join('\n');
}
