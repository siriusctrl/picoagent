import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createAppBootstrap } from '../../src/app/bootstrap.js';

test('tool registry equips ask and exec with different tool subsets', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-config-'));
  writeFileSync(join(root, 'config.md'), '---\nprovider: openai\nmodel: gpt-4o\n---\n', 'utf8');

  const previousKey = process.env.OPENAI_API_KEY;
  process.env.OPENAI_API_KEY = process.env.OPENAI_API_KEY || 'test-key';
  const app = createAppBootstrap(root);
  const askTools = app.registry.forMode('ask').map((tool) => tool.name);
  const execTools = app.registry.forMode('exec').map((tool) => tool.name);

  assert.deepEqual(askTools, ['list_files', 'read_file', 'search_text']);
  assert.deepEqual(execTools, ['list_files', 'read_file', 'search_text', 'write_file', 'run_command']);

  if (previousKey === undefined) {
    delete process.env.OPENAI_API_KEY;
  } else {
    process.env.OPENAI_API_KEY = previousKey;
  }
  rmSync(root, { recursive: true, force: true });
});
