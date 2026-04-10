#!/usr/bin/env node
import type http from 'node:http';
import { parseCliArgs, usage } from './args.js';
import { startSessionServer } from '../../http/session-server.js';
import { startHttpServer } from '../../http/server.js';
import { startFilespaceServer } from '../../http/filespace-server.js';
import { HttpSessionStore } from '../../runtime/http-session-store.js';
import { loadRuntimeMounts } from '../../runtime/mount-loader.js';

function getServerUrl(server: http.Server, hostname: string): string {
  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('Expected an inet server address');
  }

  const host = hostname === '0.0.0.0' ? '127.0.0.1' : hostname;
  return `http://${host}:${address.port}`;
}

async function main(): Promise<void> {
  try {
    const command = parseCliArgs(process.argv.slice(2));

    switch (command.type) {
      case 'help':
        process.stdout.write(`${usage()}\n`);
        return;
      case 'serve': {
        const mounts = await loadRuntimeMounts(command.mounts, process.cwd());
        const server = await startHttpServer({
          cwd: process.cwd(),
          hostname: command.hostname,
          port: command.port,
          mounts,
          sessionStore: command.session ? new HttpSessionStore(command.session) : undefined,
        });
        const serverUrl = getServerUrl(server, command.hostname);

        if (command.mounts.length === 0) {
          process.stdout.write(`Listening on ${serverUrl}\n`);
        } else {
          const mountSummary = command.mounts.map((mount) => `${mount.label}=${mount.source}`).join(', ');
          process.stdout.write(`Listening on ${serverUrl} with mounts: ${mountSummary}\n`);
        }
        return;
      }
      case 'session-serve': {
        const server = await startSessionServer({
          cwd: command.root,
          hostname: command.hostname,
          port: command.port,
        });
        const serverUrl = getServerUrl(server, command.hostname);
        process.stdout.write(`Session service listening on ${serverUrl}\n`);
        return;
      }
      case 'filespace-serve': {
        const server = await startFilespaceServer({
          name: command.name,
          root: command.root,
          hostname: command.hostname,
          port: command.port,
        });
        const serverUrl = getServerUrl(server, command.hostname);
        process.stdout.write(`Filespace '${command.name}' listening on ${serverUrl}\n`);
        process.stdout.write(`Mount with: --mount ${command.name}=${serverUrl}\n`);
        return;
      }
    }
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`${message}\n\n${usage()}\n`);
    process.exitCode = 1;
  }
}

void main();
