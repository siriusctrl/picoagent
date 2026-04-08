import * as acp from '@agentclientprotocol/sdk';
import { Readable, Writable } from 'node:stream';
import { PicoAgent } from './session-agent.js';

try {
  const input = Writable.toWeb(process.stdout) as WritableStream<Uint8Array>;
  const output = Readable.toWeb(process.stdin) as unknown as ReadableStream<Uint8Array>;
  const stream = acp.ndJsonStream(input, output);

  new acp.AgentSideConnection((connection) => new PicoAgent(connection), stream);
} catch (error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`ACP agent failed to start: ${message}`);
  process.exit(1);
}
