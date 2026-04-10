import http from 'node:http';
import { createAdaptorServer } from '@hono/node-server';

export async function startNodeFetchServer(
  fetch: (request: Request) => Response | Promise<Response>,
  hostname: string,
  port: number,
): Promise<http.Server> {
  const server = createAdaptorServer({
    fetch,
    hostname,
    port,
  }) as unknown as http.Server;

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, hostname, () => {
      server.off('error', reject);
      resolve();
    });
  });

  return server;
}
