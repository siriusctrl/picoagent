function normalizeSlashes(value: string): string {
  return value.replace(/\\/g, '/');
}

function splitSegments(value: string): string[] {
  return normalizeSlashes(value).split('/').filter((segment) => segment.length > 0);
}

export function isAbsolutePath(value: string): boolean {
  return normalizeSlashes(value).startsWith('/');
}

export function normalizePath(value: string): string {
  const normalized = normalizeSlashes(value);
  const absolute = normalized.startsWith('/');
  const segments: string[] = [];

  for (const segment of normalized.split('/')) {
    if (!segment || segment === '.') {
      continue;
    }

    if (segment === '..') {
      const previous = segments.at(-1);
      if (previous && previous !== '..') {
        segments.pop();
      } else if (!absolute) {
        segments.push('..');
      }
      continue;
    }

    segments.push(segment);
  }

  if (absolute) {
    return segments.length > 0 ? `/${segments.join('/')}` : '/';
  }

  return segments.join('/') || '.';
}

export function joinPath(...parts: string[]): string {
  const filtered = parts.filter((part) => part.length > 0);
  if (filtered.length === 0) {
    return '.';
  }

  return normalizePath(filtered.join('/'));
}

export function resolvePath(...parts: string[]): string {
  let resolved = '';
  let absolute = false;

  for (let index = parts.length - 1; index >= 0; index -= 1) {
    const part = parts[index];
    if (!part) {
      continue;
    }

    const normalized = normalizeSlashes(part);
    resolved = resolved ? `${normalized}/${resolved}` : normalized;
    if (normalized.startsWith('/')) {
      absolute = true;
      break;
    }
  }

  if (!absolute) {
    const cwd = normalizeSlashes(process.cwd());
    resolved = resolved ? `${cwd}/${resolved}` : cwd;
  }

  return normalizePath(resolved);
}

export function relativePath(from: string, to: string): string {
  const left = normalizePath(from);
  const right = normalizePath(to);

  if (left === right) {
    return '';
  }

  if (isAbsolutePath(left) !== isAbsolutePath(right)) {
    return right;
  }

  const leftSegments = splitSegments(left);
  const rightSegments = splitSegments(right);
  let shared = 0;

  while (
    shared < leftSegments.length
    && shared < rightSegments.length
    && leftSegments[shared] === rightSegments[shared]
  ) {
    shared += 1;
  }

  const result = [
    ...new Array(leftSegments.length - shared).fill('..'),
    ...rightSegments.slice(shared),
  ].join('/');

  return result || '';
}

export function dirnamePath(value: string): string {
  const normalized = normalizePath(value);
  if (normalized === '/' || normalized === '.') {
    return normalized;
  }

  const index = normalized.lastIndexOf('/');
  if (index === -1) {
    return '.';
  }

  if (index === 0) {
    return '/';
  }

  return normalized.slice(0, index);
}

export function basenamePath(value: string): string {
  const normalized = normalizePath(value);
  if (normalized === '/') {
    return '/';
  }

  const index = normalized.lastIndexOf('/');
  return index === -1 ? normalized : normalized.slice(index + 1);
}

export function extnamePath(value: string): string {
  const basename = basenamePath(value);
  const index = basename.lastIndexOf('.');

  if (index <= 0) {
    return '';
  }

  return basename.slice(index);
}
