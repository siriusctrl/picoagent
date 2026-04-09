#!/usr/bin/env node
import { parseCliArgs, usage } from './args.js';
import { startHttpServer } from '../../http/server.js';

async function main(): Promise<void> {
  try {
    const command = parseCliArgs(process.argv.slice(2));

    switch (command.type) {
      case 'help':
        process.stdout.write(`${usage()}\n`);
        return;
      case 'serve': {
        await startHttpServer({
          cwd: process.cwd(),
          hostname: command.hostname,
          port: command.port,
        });
        process.stdout.write(`Listening on http://${command.hostname}:${command.port}\n`);
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
