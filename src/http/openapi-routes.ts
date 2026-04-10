import { createRoute, z } from '@hono/zod-openapi';
import {
  AcceptedRunSchema,
  CompactSessionRequestSchema,
  CompactSessionResponseSchema,
  CreateRunRequestSchema,
  CreateSessionRequestSchema,
  ErrorSchema,
  RunEventsSchema,
  RunIdParamsSchema,
  RunSnapshotSchema,
  SessionIdParamsSchema,
  SessionResourceListSchema,
  SessionResourceParamsSchema,
  SessionResourcesQuerySchema,
  SessionSnapshotSchema,
  SessionSummarySchema,
  SetSessionAgentRequestSchema,
  jsonContent,
  plainTextContent,
  textEventStreamContent,
} from './openapi-schemas.js';

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
