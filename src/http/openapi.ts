import { createRoute, OpenAPIHono, z } from '@hono/zod-openapi';

const agentError = (issue: { input: unknown }) =>
  issue.input === undefined ? 'agent is required' : `Unsupported agent: ${String(issue.input)}`;

const nonNegativeIntegerMessage = 'keepLastMessages must be a non-negative integer';

export const AgentSchema = z.enum(['ask', 'exec'], { error: agentError });
export const OptionalAgentSchema = AgentSchema.optional();

const PromptSchema = z.string({ error: 'prompt is required' }).trim().min(1, 'prompt is required');

const ErrorSchema = z.object({
  error: z.string(),
});

const RunStatusSchema = z.enum(['running', 'completed', 'failed']);

const ControlConfigSchema = z.object({
  provider: z.enum(['anthropic', 'openai', 'gemini', 'echo']),
  model: z.string(),
  maxTokens: z.number(),
  contextWindow: z.number(),
  baseURL: z.string().optional(),
});

const RunSnapshotSchema = z.object({
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

const SessionSummarySchema = z.object({
  id: z.string(),
  agent: AgentSchema,
  cwd: z.string(),
  controlVersion: z.string(),
  controlConfig: ControlConfigSchema,
  checkpointCount: z.number(),
  createdAt: z.string(),
});

const SessionSnapshotSchema = z.object({
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

const RunEventsSchema = z.object({
  runId: z.string(),
  status: RunStatusSchema,
  events: z.array(z.record(z.string(), z.unknown())),
});

const CreateRunRequestSchema = z.object({
  prompt: PromptSchema,
  agent: OptionalAgentSchema,
});

const CreateSessionRequestSchema = z.object({
  agent: OptionalAgentSchema,
});

const SetSessionAgentRequestSchema = z.object({
  agent: AgentSchema,
});

const CompactSessionRequestSchema = z.object({
  keepLastMessages: z
    .number({ error: nonNegativeIntegerMessage })
    .int(nonNegativeIntegerMessage)
    .gte(0, nonNegativeIntegerMessage)
    .optional(),
});

const AcceptedRunSchema = z.object({
  runId: z.string(),
  status: RunStatusSchema,
  sessionId: z.string().optional(),
});

const SessionResourceListSchema = z.object({
  sessionId: z.string(),
  path: z.string(),
  entries: z.array(z.string()),
});

const CheckpointSchema = z.object({
  checkpointId: z.string(),
  summary: z.string(),
  compactedMessages: z.number(),
  keptMessages: z.number(),
});

const CompactSessionResponseSchema = z.object({
  checkpoint: CheckpointSchema,
  session: SessionSnapshotSchema,
});

const RunIdParamsSchema = z.object({
  runId: z.string(),
});

const SessionIdParamsSchema = z.object({
  sessionId: z.string(),
});

const SessionResourceParamsSchema = z.object({
  sessionId: z.string(),
  resourcePath: z.string(),
});

const SessionResourcesQuerySchema = z.object({
  path: z.string().optional(),
});

const plainTextContent = {
  'text/plain': {
    schema: z.string(),
  },
};

const jsonContent = <Schema extends z.ZodTypeAny>(schema: Schema) => ({
  'application/json': {
    schema,
  },
});

const textEventStreamContent = {
  'text/event-stream': {
    schema: z.string(),
  },
};

export const createStandaloneRunRoute = createRoute({
  method: 'post',
  path: '/runs',
  summary: 'Create one standalone run',
  description: 'Starts an async run without a persistent session and returns a run id immediately.',
  request: {
    body: {
      required: true,
      content: jsonContent(CreateRunRequestSchema),
    },
  },
  responses: {
    202: {
      description: 'Accepted run',
      content: jsonContent(AcceptedRunSchema),
    },
    400: {
      description: 'Invalid request',
      content: jsonContent(ErrorSchema),
    },
    500: {
      description: 'Server error',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const getRunRoute = createRoute({
  method: 'get',
  path: '/runs/:runId',
  summary: 'Get one run snapshot',
  request: {
    params: RunIdParamsSchema,
  },
  responses: {
    200: {
      description: 'Run snapshot',
      content: jsonContent(RunSnapshotSchema),
    },
    404: {
      description: 'Unknown run',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const getRunEventsRoute = createRoute({
  method: 'get',
  path: '/events/:runId',
  summary: 'Read or stream one run event log',
  description:
    'Default response is JSON with the complete event array. Set Accept: text/event-stream to receive the same events over SSE.',
  request: {
    params: RunIdParamsSchema,
  },
  responses: {
    200: {
      description: 'Run events as JSON or SSE',
      content: {
        ...jsonContent(RunEventsSchema),
        ...textEventStreamContent,
      },
    },
    404: {
      description: 'Unknown run',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const createSessionRoute = createRoute({
  method: 'post',
  path: '/sessions',
  summary: 'Create one persistent session',
  description:
    'Sessions preserve context across multiple runs, bind to one workspace root, and cache the resolved control snapshot used for future session runs.',
  request: {
    body: {
      required: false,
      content: jsonContent(CreateSessionRequestSchema),
    },
  },
  responses: {
    201: {
      description: 'Created session metadata',
      content: jsonContent(SessionSummarySchema),
    },
    400: {
      description: 'Invalid request',
      content: jsonContent(ErrorSchema),
    },
    500: {
      description: 'Server error',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const getSessionRoute = createRoute({
  method: 'get',
  path: '/sessions/:sessionId',
  summary: 'Get one session snapshot',
  request: {
    params: SessionIdParamsSchema,
  },
  responses: {
    200: {
      description: 'Session snapshot including ordered runs',
      content: jsonContent(SessionSnapshotSchema),
    },
    404: {
      description: 'Unknown session',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const createSessionRunRoute = createRoute({
  method: 'post',
  path: '/sessions/:sessionId/runs',
  summary: 'Create one run inside a session',
  description:
    'Starts an async run that appends to the session context after successful completion. When agent is omitted, the run inherits the session default agent preset. The server refreshes the session control snapshot automatically if the bound workspace changed.',
  request: {
    params: SessionIdParamsSchema,
    body: {
      required: true,
      content: jsonContent(CreateRunRequestSchema),
    },
  },
  responses: {
    202: {
      description: 'Accepted run',
      content: jsonContent(AcceptedRunSchema),
    },
    400: {
      description: 'Invalid request',
      content: jsonContent(ErrorSchema),
    },
    404: {
      description: 'Unknown session',
      content: jsonContent(ErrorSchema),
    },
    409: {
      description: 'Session already running',
      content: jsonContent(ErrorSchema),
    },
    500: {
      description: 'Server error',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const listSessionResourcesRoute = createRoute({
  method: 'get',
  path: '/sessions/:sessionId/resources',
  summary: 'List session history resources',
  request: {
    params: SessionIdParamsSchema,
    query: SessionResourcesQuerySchema,
  },
  responses: {
    200: {
      description: 'Session resource directory listing',
      content: jsonContent(SessionResourceListSchema),
    },
    404: {
      description: 'Unknown session or resource path',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const readSessionResourceRoute = createRoute({
  method: 'get',
  path: '/sessions/:sessionId/resources/:resourcePath{.+}',
  summary: 'Read one session history resource',
  request: {
    params: SessionResourceParamsSchema,
  },
  responses: {
    200: {
      description: 'Session resource content',
      content: {
        ...plainTextContent,
        'application/x-ndjson': {
          schema: z.string(),
        },
      },
    },
    404: {
      description: 'Unknown session or resource path',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const setSessionAgentRoute = createRoute({
  method: 'post',
  path: '/sessions/:sessionId/agent',
  summary: 'Update the default agent preset for a session',
  request: {
    params: SessionIdParamsSchema,
    body: {
      required: true,
      content: jsonContent(SetSessionAgentRequestSchema),
    },
  },
  responses: {
    200: {
      description: 'Updated session snapshot',
      content: jsonContent(SessionSnapshotSchema),
    },
    400: {
      description: 'Invalid request',
      content: jsonContent(ErrorSchema),
    },
    404: {
      description: 'Unknown session',
      content: jsonContent(ErrorSchema),
    },
    409: {
      description: 'Session already running',
      content: jsonContent(ErrorSchema),
    },
  },
});

export const compactSessionRoute = createRoute({
  method: 'post',
  path: '/sessions/:sessionId/compact',
  summary: 'Compact a session into a checkpoint plus live tail',
  request: {
    params: SessionIdParamsSchema,
    body: {
      required: false,
      content: jsonContent(CompactSessionRequestSchema),
    },
  },
  responses: {
    200: {
      description: 'Compacted session snapshot',
      content: jsonContent(CompactSessionResponseSchema),
    },
    400: {
      description: 'Invalid request',
      content: jsonContent(ErrorSchema),
    },
    404: {
      description: 'Unknown session',
      content: jsonContent(ErrorSchema),
    },
    409: {
      description: 'Session already running',
      content: jsonContent(ErrorSchema),
    },
  },
});

function normalizeOpenApiPath(path: string): string {
  return path
    .replace(/:([A-Za-z0-9_]+)\{[^}]+\}/g, '{$1}')
    .replace(/:([A-Za-z0-9_]+)/g, '{$1}');
}

export function buildOpenApiDocument(app: OpenAPIHono): Record<string, unknown> {
  const document = app.getOpenAPI31Document({
    openapi: '3.1.0',
    info: {
      title: 'Picoagent HTTP API',
      version: '0.1.0',
      description:
        'Async-first Picoagent HTTP API. Sessions preserve context, runs represent one execution, and events expose execution history or an SSE stream.',
    },
  }) as unknown as Record<string, unknown> & { paths?: Record<string, unknown> };

  if (!document.paths) {
    return document;
  }

  document.paths = Object.fromEntries(
    Object.entries(document.paths).map(([path, definition]) => [normalizeOpenApiPath(path), definition]),
  );
  return document;
}
