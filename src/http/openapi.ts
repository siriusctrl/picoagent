export function buildOpenApiDocument(): Record<string, unknown> {
  const runStatusSchema = {
    type: 'string',
    enum: ['running', 'completed', 'failed'],
  };

  const controlConfigSchema = {
    type: 'object',
    required: ['provider', 'model', 'maxTokens', 'contextWindow'],
    properties: {
      provider: { type: 'string', enum: ['anthropic', 'openai', 'gemini', 'echo'] },
      model: { type: 'string' },
      maxTokens: { type: 'number' },
      contextWindow: { type: 'number' },
      baseURL: { type: 'string' },
    },
  };

  const checkpointSchema = {
    type: 'object',
    required: ['checkpointId', 'summary', 'compactedMessages', 'keptMessages'],
    properties: {
      checkpointId: { type: 'string' },
      summary: { type: 'string' },
      compactedMessages: { type: 'number' },
      keptMessages: { type: 'number' },
    },
  };

  const runSnapshotSchema = {
    type: 'object',
    required: ['id', 'agent', 'status', 'prompt', 'output', 'createdAt'],
    properties: {
      id: { type: 'string' },
      sessionId: { type: 'string' },
      agent: { type: 'string', enum: ['ask', 'exec'] },
      status: runStatusSchema,
      prompt: { type: 'string' },
      output: { type: 'string' },
      error: { type: 'string' },
      createdAt: { type: 'string', format: 'date-time' },
      startedAt: { type: 'string', format: 'date-time' },
      finishedAt: { type: 'string', format: 'date-time' },
    },
  };

  const sessionSummarySchema = {
    type: 'object',
    required: ['id', 'agent', 'cwd', 'controlVersion', 'controlConfig', 'checkpointCount', 'createdAt'],
    properties: {
      id: { type: 'string' },
      agent: { type: 'string', enum: ['ask', 'exec'] },
      cwd: { type: 'string' },
      controlVersion: { type: 'string' },
      controlConfig: controlConfigSchema,
      checkpointCount: { type: 'number' },
      createdAt: { type: 'string', format: 'date-time' },
    },
  };

  const sessionSnapshotSchema = {
    type: 'object',
    required: ['id', 'cwd', 'agent', 'controlVersion', 'controlConfig', 'checkpointCount', 'createdAt', 'runs'],
    properties: {
      id: { type: 'string' },
      cwd: { type: 'string' },
      agent: { type: 'string', enum: ['ask', 'exec'] },
      controlVersion: { type: 'string' },
      controlConfig: controlConfigSchema,
      createdAt: { type: 'string', format: 'date-time' },
      activeRunId: { type: 'string' },
      activeCheckpointId: { type: 'string' },
      checkpointCount: { type: 'number' },
      runs: {
        type: 'array',
        items: runSnapshotSchema,
      },
    },
  };

  return {
    openapi: '3.1.0',
    info: {
      title: 'Picoagent HTTP API',
      version: '0.1.0',
      description:
        'Async-first Picoagent HTTP API. Sessions preserve context, runs represent one execution, and events expose execution history or an SSE stream.',
    },
    paths: {
      '/openapi.json': {
        get: {
          summary: 'Fetch the OpenAPI document for the Pico HTTP server',
          responses: {
            '200': {
              description: 'OpenAPI document',
              content: {
                'application/json': {},
              },
            },
          },
        },
      },
      '/runs': {
        post: {
          summary: 'Create one standalone run',
          description: 'Starts an async run without a persistent session and returns a run id immediately.',
          requestBody: {
            required: true,
            content: {
              'application/json': {
                schema: {
                  type: 'object',
                  required: ['prompt'],
                  properties: {
                    prompt: { type: 'string' },
                    agent: { type: 'string', enum: ['ask', 'exec'] },
                  },
                },
              },
            },
          },
          responses: {
            '202': {
              description: 'Accepted run',
              content: {
                'application/json': {
                  schema: {
                    type: 'object',
                    required: ['runId', 'status'],
                    properties: {
                      runId: { type: 'string' },
                      status: runStatusSchema,
                    },
                  },
                },
              },
            },
          },
        },
      },
      '/runs/{runId}': {
        get: {
          summary: 'Get one run snapshot',
          parameters: [
            {
              name: 'runId',
              in: 'path',
              required: true,
              schema: { type: 'string' },
            },
          ],
          responses: {
            '200': {
              description: 'Run snapshot',
              content: {
                'application/json': {
                  schema: runSnapshotSchema,
                },
              },
            },
            '404': {
              description: 'Unknown run',
            },
          },
        },
      },
      '/events/{runId}': {
        get: {
          summary: 'Read or stream one run event log',
          description:
            'Default response is JSON with the complete event array. Set Accept: text/event-stream to receive the same events over SSE.',
          parameters: [
            {
              name: 'runId',
              in: 'path',
              required: true,
              schema: { type: 'string' },
            },
          ],
          responses: {
            '200': {
              description: 'Run events as JSON or SSE',
              content: {
                'application/json': {
                  schema: {
                    type: 'object',
                    required: ['runId', 'status', 'events'],
                    properties: {
                      runId: { type: 'string' },
                      status: runStatusSchema,
                      events: {
                        type: 'array',
                        items: { type: 'object' },
                      },
                    },
                  },
                },
                'text/event-stream': {
                  schema: {
                    type: 'string',
                    description:
                      'SSE frames carrying run_started, assistant_delta, tool_call, tool_call_update, done, and error events.',
                  },
                },
              },
            },
            '404': {
              description: 'Unknown run',
            },
          },
        },
      },
      '/sessions': {
        post: {
          summary: 'Create one persistent session',
          description:
            'Sessions preserve context across multiple runs, bind to one workspace root, and cache the resolved control snapshot used for future session runs.',
          requestBody: {
            required: false,
            content: {
              'application/json': {
                schema: {
                  type: 'object',
                  properties: {
                    agent: { type: 'string', enum: ['ask', 'exec'] },
                  },
                },
              },
            },
          },
          responses: {
            '201': {
              description: 'Created session metadata',
              content: {
                'application/json': {
                  schema: sessionSummarySchema,
                },
              },
            },
          },
        },
      },
      '/sessions/{sessionId}': {
        get: {
          summary: 'Get one session snapshot',
          parameters: [
            {
              name: 'sessionId',
              in: 'path',
              required: true,
              schema: { type: 'string' },
            },
          ],
          responses: {
            '200': {
              description: 'Session snapshot including ordered runs',
              content: {
                'application/json': {
                  schema: sessionSnapshotSchema,
                },
              },
            },
            '404': {
              description: 'Unknown session',
            },
          },
        },
      },
      '/sessions/{sessionId}/runs': {
        post: {
          summary: 'Create one run inside a session',
          description:
            'Starts an async run that appends to the session context after successful completion. When agent is omitted, the run inherits the session default agent preset. The server refreshes the session control snapshot automatically if the bound workspace changed.',
          parameters: [
            {
              name: 'sessionId',
              in: 'path',
              required: true,
              schema: { type: 'string' },
            },
          ],
          requestBody: {
            required: true,
            content: {
              'application/json': {
                schema: {
                  type: 'object',
                  required: ['prompt'],
                  properties: {
                    prompt: { type: 'string' },
                    agent: { type: 'string', enum: ['ask', 'exec'] },
                  },
                },
              },
            },
          },
          responses: {
            '202': {
              description: 'Accepted run',
              content: {
                'application/json': {
                  schema: {
                    type: 'object',
                    required: ['runId', 'status', 'sessionId'],
                    properties: {
                      runId: { type: 'string' },
                      status: runStatusSchema,
                      sessionId: { type: 'string' },
                    },
                  },
                },
              },
            },
            '404': {
              description: 'Unknown session',
            },
            '409': {
              description: 'Session already has an active run',
            },
          },
        },
      },
      '/sessions/{sessionId}/agent': {
        post: {
          summary: 'Update one session default agent',
          parameters: [
            {
              name: 'sessionId',
              in: 'path',
              required: true,
              schema: { type: 'string' },
            },
          ],
          requestBody: {
            required: true,
            content: {
              'application/json': {
                schema: {
                  type: 'object',
                  required: ['agent'],
                  properties: {
                    agent: { type: 'string', enum: ['ask', 'exec'] },
                  },
                },
              },
            },
          },
          responses: {
            '200': {
              description: 'Updated session snapshot',
              content: {
                'application/json': {
                  schema: sessionSnapshotSchema,
                },
              },
            },
            '404': {
              description: 'Unknown session',
            },
            '409': {
              description: 'Session already has an active run',
            },
          },
        },
      },
      '/sessions/{sessionId}/compact': {
        post: {
          summary: 'Compact one session into a checkpoint plus recent tail',
          parameters: [
            {
              name: 'sessionId',
              in: 'path',
              required: true,
              schema: { type: 'string' },
            },
          ],
          requestBody: {
            required: false,
            content: {
              'application/json': {
                schema: {
                  type: 'object',
                  properties: {
                    keepLastMessages: { type: 'number' },
                  },
                },
              },
            },
          },
          responses: {
            '200': {
              description: 'Created checkpoint and updated session snapshot',
              content: {
                'application/json': {
                  schema: {
                    type: 'object',
                    required: ['checkpoint', 'session'],
                    properties: {
                      checkpoint: checkpointSchema,
                      session: sessionSnapshotSchema,
                    },
                  },
                },
              },
            },
            '404': {
              description: 'Unknown session',
            },
            '409': {
              description: 'Session already has an active run',
            },
          },
        },
      },
    },
  };
}
