import { test } from 'node:test';
import assert from 'node:assert';
import { existsSync } from 'fs';
import { mkdtempSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import { createRunWorkspace } from '../../src/lib/workspace.js';
import { gitOk } from '../../src/lib/git.js';

// Use a tmp baseDir so tests don't touch real /srv or ~/.picoagent

test('createRunWorkspace creates repo and tasks dirs and initializes git', () => {
  const baseDir = mkdtempSync(join(tmpdir(), 'pico-ws-'));
  const ws = createRunWorkspace({ baseDir });

  assert.ok(existsSync(ws.runDir));
  assert.ok(existsSync(ws.repoDir));
  assert.ok(existsSync(ws.tasksDir));
  assert.ok(existsSync(join(ws.repoDir, '.git')));

  // Should have at least one commit (seed)
  const log = gitOk(['log', '-1', '--pretty=%s'], { cwd: ws.repoDir });
  assert.ok(log.stdout.includes('init run workspace'));
});
