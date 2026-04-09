import { z } from 'zod';
import { Tool } from '../core/types.js';

const CompactSessionParams = z.object({
  keepLastMessages: z
    .number()
    .int()
    .min(0)
    .max(200)
    .optional()
    .describe('How many trailing messages to keep outside the checkpoint. Defaults to 8.'),
});

export const compactSessionTool: Tool<typeof CompactSessionParams> = {
  name: 'compact_session',
  description: 'Compact older session messages into a checkpoint summary while keeping a recent tail.',
  kind: 'edit',
  parameters: CompactSessionParams,
  title: () => 'Compact session',
  async execute(args, context) {
    if (!context.sessionId) {
      throw new Error('Session compaction requires a persistent session');
    }

    const result = await context.sessionAccess.compactSession(context.sessionId, args.keepLastMessages ?? 8);

    return {
      content: `Created checkpoint ${result.checkpointId} after compacting ${result.compactedMessages} messages and keeping ${result.keptMessages}.`,
      rawOutput: result,
    };
  },
};
