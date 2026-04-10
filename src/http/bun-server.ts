export interface LocalServerHandle {
  readonly url: URL;
  readonly port: number;
  stop(closeActiveConnections?: boolean): Promise<void>;
}

export async function startBunFetchServer(
  fetch: (request: Request) => Response | Promise<Response>,
  hostname: string,
  port: number,
): Promise<LocalServerHandle> {
  const server = Bun.serve({
    hostname,
    port,
    fetch,
  });
  const boundPort = server.port ?? Number(server.url.port);

  return {
    url: server.url,
    port: boundPort,
    stop: (closeActiveConnections = false) => server.stop(closeActiveConnections),
  };
}
