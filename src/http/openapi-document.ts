import type { OpenAPIHono } from '@hono/zod-openapi';

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
