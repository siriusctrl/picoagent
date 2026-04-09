import { z } from 'zod';
import { Tool } from '../core/types.js';

const ReadSessionResourceParams = z.object({
  path: z.string().min(1).describe('Session resource path like summary.md, runs/<id>.md, or checkpoints/<id>.md.'),
});

export const readSessionResourceTool: Tool<typeof ReadSessionResourceParams> = {
  name: 'read_session_resource',
  description: 'Read one virtual session history resource.',
  kind: 'read',
  parameters: ReadSessionResourceParams,
  title: (args) => `Read session resource ${args.path}`,
  async execute(args, context) {
    if (!context.sessionId) {
      throw new Error('Session resources require a persistent session');
    }

    const content = await context.sessionAccess.readResource(context.sessionId, args.path);

    return {
      content,
      display: [{ type: 'text', text: content }],
      rawOutput: { path: args.path },
    };
  },
};
