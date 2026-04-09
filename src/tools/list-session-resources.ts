import { z } from 'zod';
import { Tool } from '../core/types.js';

const ListSessionResourcesParams = z.object({
  path: z.string().optional().describe('Optional session resource directory. Defaults to the session root.'),
});

export const listSessionResourcesTool: Tool<typeof ListSessionResourcesParams> = {
  name: 'list_session_resources',
  description: 'List virtual session history resources such as checkpoints, runs, and event logs.',
  kind: 'search',
  parameters: ListSessionResourcesParams,
  title: (args) => `List session resources in ${args.path ?? '.'}`,
  async execute(args, context) {
    if (!context.sessionId) {
      throw new Error('Session resources require a persistent session');
    }

    const path = args.path ?? '.';
    const entries = await context.sessionAccess.listResources(context.sessionId, path);

    return {
      content: entries.length > 0 ? entries.join('\n') : 'No session resources found.',
      rawOutput: { path, count: entries.length },
    };
  },
};
