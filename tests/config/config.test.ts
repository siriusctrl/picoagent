import { test, expect } from 'bun:test';
import { loadConfig, resolveApiKey } from '../../src/config/config.ts';
import { joinPath } from '../../src/fs/path.ts';
import { ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

async function withHome<T>(homeDir: string, fn: () => Promise<T>): Promise<T> {
  const previousHome = process.env.HOME;
  process.env.HOME = homeDir;

  try {
    return await fn();
  } finally {
    if (previousHome === undefined) {
      delete process.env.HOME;
    } else {
      process.env.HOME = previousHome;
    }
  }
}

test('loadConfig falls back to built-in echo defaults when no config files exist', async () => {
  const root = await makeTempDir('picoagent-config-missing-');
  const home = await makeTempDir('picoagent-home-');

  try {
    const config = await withHome(home, () => loadConfig(root));

    expect(config).toEqual({
      provider: 'echo',
      model: 'echo',
      maxTokens: 4096,
      contextWindow: 200000,
      baseURL: undefined,
    });
  } finally {
    await removeDir(root);
    await removeDir(home);
  }
});

test('loadConfig merges ~/.pico/config.jsonc with workspace overrides from ./.pico/config.jsonc', async () => {
  const root = await makeTempDir('picoagent-config-local-');
  const home = await makeTempDir('picoagent-home-');

  try {
    await ensureDir(joinPath(home, '.pico'));
    await writeTextFile(
      joinPath(home, '.pico', 'config.jsonc'),
      '{\n  // defaults for this user\n  "provider": "openai",\n  "model": "gpt-4o-mini",\n  "maxTokens": 2048,\n}\n',
    );

    await ensureDir(joinPath(root, '.pico'));
    await writeTextFile(
      joinPath(root, '.pico', 'config.jsonc'),
      '{\n  "provider": "echo",\n  "model": "echo",\n}\n',
    );

    const config = await withHome(home, () => loadConfig(root));

    expect(config).toEqual({
      provider: 'echo',
      model: 'echo',
      maxTokens: 2048,
      contextWindow: 200000,
      baseURL: undefined,
    });
  } finally {
    await removeDir(root);
    await removeDir(home);
  }
});

test('loadConfig reports invalid JSONC clearly', async () => {
  const root = await makeTempDir('picoagent-config-invalid-');
  const home = await makeTempDir('picoagent-home-');

  try {
    await ensureDir(joinPath(root, '.pico'));
    await writeTextFile(joinPath(root, '.pico', 'config.jsonc'), '{ invalid jsonc }');

    await expect(withHome(home, () => loadConfig(root))).rejects.toThrow(/Invalid JSONC/);
  } finally {
    await removeDir(root);
    await removeDir(home);
  }
});

test('loadConfig accepts the built-in echo provider without an API key', async () => {
  const root = await makeTempDir('picoagent-config-echo-');
  const home = await makeTempDir('picoagent-home-');

  try {
    await ensureDir(joinPath(root, '.pico'));
    await writeTextFile(joinPath(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }');

    const config = await withHome(home, () => loadConfig(root));

    expect(config).toEqual({
      provider: 'echo',
      model: 'echo',
      maxTokens: 4096,
      contextWindow: 200000,
      baseURL: undefined,
    });
    expect(resolveApiKey('echo')).toBe('');
  } finally {
    await removeDir(root);
    await removeDir(home);
  }
});
