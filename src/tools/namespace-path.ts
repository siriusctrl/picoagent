import type { NamespaceLikePath } from '../core/file-view.ts';

export function parseNamespacePath(inputPath: string): { namespace: string; relativePath: string } {
  if (!inputPath.startsWith('/')) {
    throw new Error('Expected an absolute namespace path like /workspace/src/app.ts.');
  }

  const [, namespace, ...parts] = inputPath.split('/');
  if (!namespace) {
    throw new Error('Expected an absolute namespace path like /workspace/src/app.ts.');
  }

  return {
    namespace,
    relativePath: parts.length > 0 ? parts.join('/') : '.',
  };
}

export function namespacePath(namespace: string, relativePath: string): NamespaceLikePath {
  return (relativePath === '.' || relativePath === '')
    ? `/${namespace}`
    : `/${namespace}/${relativePath}` as NamespaceLikePath;
}
