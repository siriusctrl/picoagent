import { test, expect } from 'bun:test';
import { parseCliArgs, usage } from '../../../src/clients/cli/args.ts';

test('parseCliArgs defaults to help with no command', () => {
  expect(parseCliArgs([])).toEqual({ type: 'help' });
});

test('parseCliArgs parses serve', () => {
  expect(parseCliArgs(['serve'])).toEqual({
    type: 'serve',
    hostname: '127.0.0.1',
    port: 4096,
    mounts: [],
    session: undefined,
  });
});

test('parseCliArgs parses serve host and port overrides', () => {
  expect(parseCliArgs(['serve', '--hostname', '0.0.0.0', '--port', '8080'])).toEqual({
    type: 'serve',
    hostname: '0.0.0.0',
    port: 8080,
    mounts: [],
    session: undefined,
  });
});

test('parseCliArgs parses repeated mounts for serve', () => {
  expect(
    parseCliArgs(['serve', '--mount', 'docs=./', '--mount=remote@build=http://10.0.0.8:5001']),
  ).toEqual({
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
  expect(parseCliArgs(['serve', '--session', 'http://127.0.0.1:4097'])).toEqual({
    type: 'serve',
    hostname: '127.0.0.1',
    port: 4096,
    mounts: [],
    session: 'http://127.0.0.1:4097',
  });
});

test('parseCliArgs parses filespace serve', () => {
  expect(
    parseCliArgs([
      'filespace',
      'serve',
      '--hostname',
      '0.0.0.1',
      '--port',
      '5001',
      '--name',
      'build',
      '--root',
      '/data/workspace',
    ]),
  ).toEqual({
    type: 'filespace-serve',
    hostname: '0.0.0.1',
    port: 5001,
    name: 'build',
    root: '/data/workspace',
  });
});

test('parseCliArgs defaults filespace serve options', () => {
  expect(parseCliArgs(['filespace', 'serve'])).toEqual({
    type: 'filespace-serve',
    hostname: '127.0.0.1',
    port: 4096,
    name: 'filespace',
    root: process.cwd(),
  });
});

test('parseCliArgs parses session serve', () => {
  expect(
    parseCliArgs(['session', 'serve', '--hostname', '0.0.0.0', '--port', '5002', '--root', '/tmp/session-root']),
  ).toEqual({
    type: 'session-serve',
    hostname: '0.0.0.0',
    port: 5002,
    root: '/tmp/session-root',
  });
});

test('parseCliArgs rejects malformed mount values', () => {
  expect(() => parseCliArgs(['serve', '--mount', 'noval'])).toThrow(/--mount requires label=source/);
  expect(() => parseCliArgs(['serve', '--mount='])).toThrow(/--mount requires label=source/);
});

test('parseCliArgs rejects unknown commands', () => {
  expect(() => parseCliArgs(['wat'])).toThrow(/Unknown command/);
  expect(() => parseCliArgs(['run', 'hello'])).toThrow(/Unknown command/);
  expect(() => parseCliArgs(['filespace', 'build'])).toThrow(/Unknown command/);
  expect(() => parseCliArgs(['session', 'build'])).toThrow(/Unknown command/);
});

test('usage mentions the minimum command surface', () => {
  expect(usage()).toMatch(/pico serve/);
  expect(usage()).toMatch(/pico filespace serve/);
  expect(usage()).toMatch(/pico session serve/);
  expect(usage()).toMatch(/--mount/);
  expect(usage()).not.toMatch(/pico run/);
});
