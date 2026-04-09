import { z } from 'zod';
import { FilePatchChange, FilePatchOperation } from '../core/file-view.js';
import { Tool } from '../core/types.js';
import { relativeToCwd, resolveSessionPath } from '../fs/filesystem.js';

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
  target: z.enum(['workspace', 'session']).describe('Which file-view to patch.'),
  operations: z.array(PatchOperationSchema).min(1).max(50).describe('One or more patch operations.'),
});

function collectWorkspaceLocations(operations: FilePatchOperation[], cwd: string, roots: string[]) {
  return operations.map((operation) => ({
    path: resolveSessionPath(operation.path, cwd, roots),
  }));
}

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
  description: 'Apply one or more create, replace, or delete operations to a file-view.',
  kind: 'edit',
  parameters: PatchParams,
  title: (args) => `Patch ${args.target}`,
  locations: (args, context) =>
    args.target === 'workspace'
      ? collectWorkspaceLocations(args.operations as FilePatchOperation[], context.cwd, context.roots)
      : [],
  async execute(args, context) {
    const changes = await context.fileView.patch(args.target, args.operations as FilePatchOperation[]);

    return {
      content:
        changes.length === 1
          ? `${describeAction(changes[0].action)} ${relativeToCwd(changes[0].path, context.cwd)}`
          : changes.map((change) => `${change.action} ${relativeToCwd(change.path, context.cwd)}`).join('\n'),
      display: changes.map((change) => ({
        type: 'diff' as const,
        path: change.path,
        oldText: change.oldText,
        newText: change.newText ?? '',
      })),
      rawOutput: {
        target: args.target,
        count: changes.length,
      },
      locations: changes.map((change) => ({ path: change.path })),
    };
  },
};
