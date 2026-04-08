import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { buildSystemPrompt } from '../../src/lib/prompt.js';
import { searchTextTool } from '../../src/tools/search-text.js';

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

test('buildSystemPrompt keeps root prompt docs and reads memory from .pico directories', () => {
  const root = mkdtempSync(join(tmpdir(), 'picoagent-prompt-'));
  const home = mkdtempSync(join(tmpdir(), 'picoagent-home-'));

  try {
    writeFileSync(join(root, 'SOUL.md'), 'workspace soul', 'utf8');
    writeFileSync(join(root, 'USER.md'), 'workspace user', 'utf8');
    writeFileSync(join(root, 'AGENTS.md'), 'workspace agents', 'utf8');

    mkdirSync(join(root, '.pico', 'memory'), { recursive: true });
    writeFileSync(join(root, '.pico', 'memory', 'memory.md'), 'workspace memory', 'utf8');

    mkdirSync(join(home, '.pico', 'memory'), { recursive: true });
    writeFileSync(join(home, '.pico', 'memory', 'memory.md'), 'user memory', 'utf8');

    const prompt = withHome(home, () => buildSystemPrompt(root, 'ask', [searchTextTool]));

    assert.match(prompt, /workspace soul/);
    assert.match(prompt, /workspace user/);
    assert.match(prompt, /workspace agents/);
    assert.match(prompt, /user memory/);
    assert.match(prompt, /workspace memory/);
  } finally {
    rmSync(root, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
  }
});
