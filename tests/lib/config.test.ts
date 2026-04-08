import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { loadConfig, resolveApiKey } from '../../src/lib/config.js';

function withHome<T>(homeDir: string, fn: () => T): T {
  const previousHome = process.env.HOME;
  process.env.HOME = homeDir;

  try {
    return fn();
  } finally {
    if (previousHome === undefined) {
      delete process.env.HOME;
    } else {
      process.env.HOME = previousHome;
    }
  }
}

test('loadConfig falls back to built-in echo defaults when no config files exist', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-missing-'));
  const home = mkdtempSync(join(tmpdir(), 'picoagent-home-'));

  try {
    const config = withHome(home, () => loadConfig(root));

    assert.deepEqual(config, {
      provider: 'echo',
      model: 'echo',
      maxTokens: 4096,
      contextWindow: 200000,
      baseURL: undefined,
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
  }
});

test('loadConfig merges ~/.pico/config.jsonc with workspace overrides from ./.pico/config.jsonc', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-local-'));
  const home = mkdtempSync(join(tmpdir(), 'picoagent-home-'));

  try {
    mkdirSync(join(home, '.pico'), { recursive: true });
    writeFileSync(
      join(home, '.pico', 'config.jsonc'),
      '{\n  // defaults for this user\n  "provider": "openai",\n  "model": "gpt-4o-mini",\n  "maxTokens": 2048,\n}\n',
      'utf8',
    );

    mkdirSync(join(root, '.pico'), { recursive: true });
    writeFileSync(
      join(root, '.pico', 'config.jsonc'),
      '{\n  "provider": "echo",\n  "model": "echo",\n}\n',
      'utf8',
    );

    const config = withHome(home, () => loadConfig(root));

    assert.deepEqual(config, {
      provider: 'echo',
      model: 'echo',
      maxTokens: 2048,
      contextWindow: 200000,
      baseURL: undefined,
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
  }
});

test('loadConfig reports invalid JSONC clearly', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-invalid-'));
  const home = mkdtempSync(join(tmpdir(), 'picoagent-home-'));

  try {
    mkdirSync(join(root, '.pico'), { recursive: true });
    writeFileSync(join(root, '.pico', 'config.jsonc'), '{ invalid jsonc }', 'utf8');

    assert.throws(() => withHome(home, () => loadConfig(root)), /Invalid JSONC/);
  } finally {
    rmSync(root, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
  }
});

test('loadConfig accepts the built-in echo provider without an API key', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-echo-'));
  const home = mkdtempSync(join(tmpdir(), 'picoagent-home-'));

  try {
    mkdirSync(join(root, '.pico'), { recursive: true });
    writeFileSync(join(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }', 'utf8');

    const config = withHome(home, () => loadConfig(root));

    assert.deepEqual(config, {
      provider: 'echo',
      model: 'echo',
      maxTokens: 4096,
      contextWindow: 200000,
      baseURL: undefined,
    });
    assert.equal(resolveApiKey('echo'), '');
  } finally {
    rmSync(root, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
  }
});
