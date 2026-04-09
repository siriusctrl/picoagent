import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRuntimeContext } from '../../src/runtime/index.js';

test('tool registry equips ask and exec agent presets with different tool subsets', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-'));
  mkdirSync(join(root, '.pico'), { recursive: true });
  writeFileSync(join(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }\n', 'utf8');

  const runtime = createRuntimeContext(root);
  const askTools = runtime.registry.forAgent('ask').map((tool) => tool.name);
  const execTools = runtime.registry.forAgent('exec').map((tool) => tool.name);

  assert.deepEqual(askTools, ['glob', 'grep', 'read']);
  assert.deepEqual(execTools, ['glob', 'grep', 'read', 'patch', 'cmd']);

  rmSync(root, { recursive: true, force: true });
});
