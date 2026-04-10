import { z } from 'zod';
import { FilePatchChange, FilePatchOperation } from '../core/file-view.ts';
import { Tool } from '../core/types.ts';
import { resolveSessionPath } from '../fs/filesystem.ts';
import { parseNamespacePath } from './namespace-path.ts';

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
  locations: (args, context) => {
    return args.operations.flatMap((operation) => {
      const parsed = parseNamespacePath(operation.path);
      if (parsed.namespace !== 'workspace') {
        return [];
      }

      return [{ path: resolveSessionPath(parsed.relativePath, context.cwd, context.roots) }];
    });
  },
  async execute(args, context) {
    const changes = await context.fileView.patch(args.operations as FilePatchOperation[]);

    return {
      content:
        changes.length === 1
          ? `${describeAction(changes[0].action)} ${changes[0].path}`
          : changes
              .map((change) => `${change.action} ${change.path}`)
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
      locations: changes.flatMap((change) => {
        const parsed = parseNamespacePath(change.path);
        if (parsed.namespace !== 'workspace') {
          return [];
        }

        return [{ path: resolveSessionPath(parsed.relativePath, context.cwd, context.roots) }];
      }),
    };
  },
};
