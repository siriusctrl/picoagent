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

test('parseCliArgs rejects unknown commands', () => {
  assert.throws(() => parseCliArgs(['wat']), /Unknown command/);
  assert.throws(() => parseCliArgs(['run', 'hello']), /Unknown command/);
});

test('usage mentions the minimum command surface', () => {
  assert.match(usage(), /pico serve/);
  assert.doesNotMatch(usage(), /pico run/);
});
