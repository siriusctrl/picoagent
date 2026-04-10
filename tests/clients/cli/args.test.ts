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
    mounts: [],
    session: undefined,
  });
});

test('parseCliArgs parses serve host and port overrides', () => {
  assert.deepEqual(parseCliArgs(['serve', '--hostname', '0.0.0.0', '--port', '8080']), {
    type: 'serve',
    hostname: '0.0.0.0',
    port: 8080,
    mounts: [],
    session: undefined,
  });
});

test('parseCliArgs parses repeated mounts for serve', () => {
  assert.deepEqual(parseCliArgs(['serve', '--mount', 'docs=./', '--mount=remote@build=http://10.0.0.8:5001']), {
    type: 'serve',
    hostname: '127.0.0.1',
    port: 4096,
    mounts: [
      { label: 'docs', source: './' },
      { label: 'remote@build', source: 'http://10.0.0.8:5001' },
    ],
    session: undefined,
  });
});

test('parseCliArgs parses a bound session backend for serve', () => {
  assert.deepEqual(parseCliArgs(['serve', '--session', 'http://127.0.0.1:4097']), {
    type: 'serve',
    hostname: '127.0.0.1',
    port: 4096,
    mounts: [],
    session: 'http://127.0.0.1:4097',
  });
});

test('parseCliArgs parses filespace serve', () => {
  assert.deepEqual(parseCliArgs(['filespace', 'serve', '--hostname', '0.0.0.1', '--port', '5001', '--name', 'build', '--root', '/data/workspace']), {
    type: 'filespace-serve',
    hostname: '0.0.0.1',
    port: 5001,
    name: 'build',
    root: '/data/workspace',
  });
});

test('parseCliArgs defaults filespace serve options', () => {
  assert.deepEqual(parseCliArgs(['filespace', 'serve']), {
    type: 'filespace-serve',
    hostname: '127.0.0.1',
    port: 4096,
    name: 'filespace',
    root: process.cwd(),
  });
});

test('parseCliArgs parses session serve', () => {
  assert.deepEqual(parseCliArgs(['session', 'serve', '--hostname', '0.0.0.0', '--port', '5002', '--root', '/tmp/session-root']), {
    type: 'session-serve',
    hostname: '0.0.0.0',
    port: 5002,
    root: '/tmp/session-root',
  });
});

test('parseCliArgs rejects malformed mount values', () => {
  assert.throws(() => parseCliArgs(['serve', '--mount', 'noval']), /--mount requires label=source/);
  assert.throws(() => parseCliArgs(['serve', '--mount=']), /--mount requires label=source/);
});

test('parseCliArgs rejects unknown commands', () => {
  assert.throws(() => parseCliArgs(['wat']), /Unknown command/);
  assert.throws(() => parseCliArgs(['run', 'hello']), /Unknown command/);
  assert.throws(() => parseCliArgs(['filespace', 'build']), /Unknown command/);
  assert.throws(() => parseCliArgs(['session', 'build']), /Unknown command/);
});

test('usage mentions the minimum command surface', () => {
  assert.match(usage(), /pico serve/);
  assert.match(usage(), /pico filespace serve/);
  assert.match(usage(), /pico session serve/);
  assert.match(usage(), /--mount/);
  assert.doesNotMatch(usage(), /pico run/);
});
