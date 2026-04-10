import { z } from 'zod';
import { Tool } from '../core/types.js';
import { formatLineNumberedText } from '../fs/filesystem.js';

type FileViewTarget = 'workspace' | 'session';

function parseNamespacePath(inputPath: string): { target: FileViewTarget; path: string } {
  if (!inputPath.startsWith('/')) {
    throw new Error(`Expected a namespace path, for example '/workspace/src/app.ts'.`);
  }

  const [, namespace, ...parts] = inputPath.split('/');
  if (!namespace) {
    throw new Error(`Expected a namespace path, for example '/workspace/src/app.ts'.`);
  }

  const normalized = namespace.split('@').at(-1);
  if (normalized !== 'workspace' && normalized !== 'session') {
    throw new Error(`Unsupported namespace '${namespace}'.`);
  }

  return {
    target: normalized,
    path: parts.length ? parts.join('/') : '.',
  };
}

function namespacePath(target: FileViewTarget, relativePath: string): string {
  if (relativePath === '.') {
    return `/${target}`;
  }

  if (relativePath.startsWith('/')) {
    return relativePath;
  }

  return `/${target}/${relativePath}`;
}

const ReadParams = z.object({
  path: z
    .string()
    .min(1)
    .describe('Namespace path to read, for example /workspace/src/app.ts or /session/summary.md.'),
  line: z.number().int().positive().optional().describe('Optional starting line number.'),
  limit: z.number().int().positive().max(500).optional().describe('Optional number of lines to read.'),
});

export const readTool: Tool<typeof ReadParams> = {
  name: 'read',
  description: 'Read a file from a namespace path.',
  kind: 'read',
  parameters: ReadParams,
  title: (args) => `Read ${args.path}`,
  locations: (args) => {
    const parsed = parseNamespacePath(args.path);
    return [{ path: namespacePath(parsed.target, parsed.path), line: args.line }];
  },
  async execute(args, context) {
    const parsed = parseNamespacePath(args.path);
    const content = await context.fileView.read(parsed.target, parsed.path, {
      line: args.line,
      limit: args.limit,
    });

    return {
      content: formatLineNumberedText(content, args.line ?? 1),
      display: [{ type: 'text', text: content }],
      rawOutput: {
        path: namespacePath(parsed.target, parsed.path),
      },
    };
  },
};
