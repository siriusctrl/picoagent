import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseCliArgs, usage } from '../../../src/clients/cli/args.js';

test('parseCliArgs defaults to help with no command', () => {
  assert.deepEqual(parseCliArgs([]), { type: 'help' });
});

test('parseCliArgs parses serve', () => {
  assert.deepEqual(parseCliArgs(['serve']), {
    type: 'serve',
    hostname: '127.0.0.1',
    port: 4096,
  });
});

test('parseCliArgs parses serve host and port overrides', () => {
  assert.deepEqual(parseCliArgs(['serve', '--hostname', '0.0.0.0', '--port', '8080']), {
    type: 'serve',
    hostname: '0.0.0.0',
    port: 8080,
  });
});

test('parseCliArgs parses run with agent and prompt', () => {
  assert.deepEqual(parseCliArgs(['run', '--agent', 'exec', 'fix', 'the', 'bug']), {
    type: 'run',
    agent: 'exec',
    prompt: 'fix the bug',
  });
});

test('parseCliArgs accepts --agent=ask form', () => {
  assert.deepEqual(parseCliArgs(['run', '--agent=ask', 'hello']), {
    type: 'run',
    agent: 'ask',
    prompt: 'hello',
  });
});

test('parseCliArgs rejects unknown commands and invalid agents', () => {
  assert.throws(() => parseCliArgs(['wat']), /Unknown command/);
  assert.throws(() => parseCliArgs(['run', '--agent', 'plan']), /Unsupported agent/);
});

test('usage mentions the minimum command surface', () => {
  assert.match(usage(), /pico serve/);
  assert.match(usage(), /pico run/);
});
