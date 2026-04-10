import { z } from '@hono/zod-openapi';

const agentError = (issue: { input: unknown }) =>
  issue.input === undefined ? 'agent is required' : `Unsupported agent: ${String(issue.input)}`;

const nonNegativeIntegerMessage = 'keepLastMessages must be a non-negative integer';

export const AgentSchema = z.enum(['ask', 'exec'], { error: agentError });
export const OptionalAgentSchema = AgentSchema.optional();

const PromptSchema = z.string({ error: 'prompt is required' }).trim().min(1, 'prompt is required');

export const ErrorSchema = z.object({
  error: z.string(),
});

export const RunStatusSchema = z.enum(['running', 'completed', 'failed']);

export const ControlConfigSchema = z.object({
  provider: z.enum(['anthropic', 'openai', 'gemini', 'echo']),
  model: z.string(),
  maxTokens: z.number(),
  contextWindow: z.number(),
  baseURL: z.string().optional(),
});

export const RunSnapshotSchema = z.object({
  id: z.string(),
  sessionId: z.string().optional(),
  agent: AgentSchema,
  status: RunStatusSchema,
  prompt: z.string(),
  output: z.string(),
  error: z.string().optional(),
  createdAt: z.string(),
  startedAt: z.string().optional(),
  finishedAt: z.string().optional(),
});

export const SessionSummarySchema = z.object({
  id: z.string(),
  agent: AgentSchema,
  cwd: z.string(),
  controlVersion: z.string(),
  controlConfig: ControlConfigSchema,
  checkpointCount: z.number(),
  createdAt: z.string(),
});

export const SessionSnapshotSchema = z.object({
  id: z.string(),
  cwd: z.string(),
  agent: AgentSchema,
  controlVersion: z.string(),
  controlConfig: ControlConfigSchema,
  createdAt: z.string(),
  activeRunId: z.string().optional(),
  activeCheckpointId: z.string().optional(),
  checkpointCount: z.number(),
  runs: z.array(RunSnapshotSchema),
});

export const RunEventsSchema = z.object({
  runId: z.string(),
  status: RunStatusSchema,
  events: z.array(z.record(z.string(), z.unknown())),
});

export const CreateRunRequestSchema = z.object({
  prompt: PromptSchema,
  agent: OptionalAgentSchema,
});

export const CreateSessionRequestSchema = z.object({
  agent: OptionalAgentSchema,
});

export const SetSessionAgentRequestSchema = z.object({
  agent: AgentSchema,
});

export const CompactSessionRequestSchema = z.object({
  keepLastMessages: z
    .number({ error: nonNegativeIntegerMessage })
    .int(nonNegativeIntegerMessage)
    .gte(0, nonNegativeIntegerMessage)
    .optional(),
});

export const AcceptedRunSchema = z.object({
  runId: z.string(),
  status: RunStatusSchema,
  sessionId: z.string().optional(),
});

export const SessionResourceListSchema = z.object({
  sessionId: z.string(),
  path: z.string(),
  entries: z.array(z.string()),
});

export const CheckpointSchema = z.object({
  checkpointId: z.string(),
  summary: z.string(),
  compactedMessages: z.number(),
  keptMessages: z.number(),
});

export const CompactSessionResponseSchema = z.object({
  checkpoint: CheckpointSchema,
  session: SessionSnapshotSchema,
});

export const RunIdParamsSchema = z.object({
  runId: z.string(),
});

export const SessionIdParamsSchema = z.object({
  sessionId: z.string(),
});

export const SessionResourceParamsSchema = z.object({
  sessionId: z.string(),
  resourcePath: z.string(),
});

export const SessionResourcesQuerySchema = z.object({
  path: z.string().optional(),
});

export const plainTextContent = {
  'text/plain': {
    schema: z.string(),
  },
};

export const textEventStreamContent = {
  'text/event-stream': {
    schema: z.string(),
  },
};

export const jsonContent = <Schema extends z.ZodTypeAny>(schema: Schema) => ({
  'application/json': {
    schema,
  },
});
