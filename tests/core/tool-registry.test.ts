import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createAppBootstrap } from '../../src/bootstrap/index.js';

test('tool registry equips ask and exec with different tool subsets', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-'));
  mkdirSync(join(root, '.pico'), { recursive: true });
  writeFileSync(join(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }\n', 'utf8');

  const app = createAppBootstrap(root);
  const askTools = app.registry.forMode('ask').map((tool) => tool.name);
  const execTools = app.registry.forMode('exec').map((tool) => tool.name);

  assert.deepEqual(askTools, ['list_files', 'read_file', 'search_text']);
  assert.deepEqual(execTools, ['list_files', 'read_file', 'search_text', 'write_file', 'run_command']);

  rmSync(root, { recursive: true, force: true });
});
