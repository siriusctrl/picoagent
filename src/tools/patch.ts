import { z } from 'zod';
import { FilePatchChange, FilePatchOperation } from '../core/file-view.js';
import { Tool } from '../core/types.js';

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
  if (relativePath.startsWith('/')) {
    return relativePath;
  }

  if (relativePath === '.') {
    return `/${target}`;
  }

  return `/${target}/${relativePath}`;
}

const PatchOperationSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('create'),
    path: z.string().min(1),
    content: z.string(),
  }),
  z.object({
    type: z.literal('replace'),
    path: z.string().min(1),
    oldText: z.string(),
    newText: z.string(),
    all: z.boolean().optional(),
  }),
  z.object({
    type: z.literal('delete'),
    path: z.string().min(1),
  }),
]);

const PatchParams = z.object({
  operations: z
    .array(PatchOperationSchema)
    .min(1)
    .max(50)
    .describe('One or more patch operations using namespace paths.'),
});

function describeAction(action: FilePatchChange['action']): string {
  if (action === 'create') {
    return 'created';
  }

  if (action === 'update') {
    return 'updated';
  }

  return 'deleted';
}

export const patchTool: Tool<typeof PatchParams> = {
  name: 'patch',
  description: 'Apply one or more create, replace, or delete operations to namespace paths.',
  kind: 'edit',
  parameters: PatchParams,
  title: () => 'Patch',
  locations: (args) => {
    const first = parseNamespacePath(args.operations[0].path);
    return args.operations.map((operation) => {
      const parsed = parseNamespacePath(operation.path);
      if (parsed.target !== first.target) {
        throw new Error('All patch operations must target the same namespace.');
      }

      return { path: namespacePath(parsed.target, parsed.path) };
    });
  },
  async execute(args, context) {
    const first = parseNamespacePath(args.operations[0].path);
    const operations = args.operations.map((operation) => {
      const parsed = parseNamespacePath(operation.path);
      if (parsed.target !== first.target) {
        throw new Error('All patch operations must target the same namespace.');
      }

      return {
        ...operation,
        path: parsed.path,
      };
    });

    const changes = await context.fileView.patch(first.target, operations as FilePatchOperation[]);

    return {
      content:
        changes.length === 1
          ? `${describeAction(changes[0].action)} ${namespacePath(first.target, changes[0].path)}`
          : changes
              .map((change) => `${change.action} ${namespacePath(first.target, change.path)}`)
              .join('\n'),
      display: changes.map((change) => ({
        type: 'diff' as const,
        path: change.path,
        oldText: change.oldText,
        newText: change.newText ?? '',
      })),
      rawOutput: {
        count: changes.length,
      },
      locations: changes.map((change) => ({ path: namespacePath(first.target, change.path) })),
    };
  },
};
