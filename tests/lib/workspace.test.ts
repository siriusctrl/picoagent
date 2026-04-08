import { test } from 'node:test';
import assert from 'node:assert';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import { createRunWorkspace } from '../../src/lib/workspace.js';
import { gitOk } from '../../src/lib/git.js';

function makeTempDir(prefix: string): string {
  return mkdtempSync(join(tmpdir(), prefix));
}

function initGitRepo(dir: string): void {
  gitOk(['init', '-q'], { cwd: dir });
  gitOk(['config', 'user.email', 'picoagent-test@local'], { cwd: dir });
  gitOk(['config', 'user.name', 'picoagent-test'], { cwd: dir });
  writeFileSync(join(dir, 'config.md'), '---\nprovider: openai\n---\n', 'utf8');
  writeFileSync(join(dir, 'README.md'), '# temp repo\n', 'utf8');
  gitOk(['add', '.'], { cwd: dir });
  gitOk(['commit', '-q', '-m', 'test: seed repo'], { cwd: dir });
}

test('createRunWorkspace attaches to the existing git repository when available', () => {
  const baseDir = makeTempDir('pico-ws-base-');
  const controlDir = makeTempDir('pico-ws-control-');
  initGitRepo(controlDir);

  const ws = createRunWorkspace({ baseDir, controlDir });

  assert.strictEqual(ws.mode, 'attached-git');
  assert.strictEqual(ws.repoDir, controlDir);
  assert.ok(existsSync(ws.runDir));
  assert.ok(existsSync(ws.tasksDir));
  assert.ok(!existsSync(join(ws.runDir, 'repo')));
});

test('createRunWorkspace creates an isolated git snapshot for non-git control directories', () => {
  const baseDir = makeTempDir('pico-ws-base-');
  const controlDir = makeTempDir('pico-ws-control-');
  mkdirSync(join(controlDir, 'memory'));
  writeFileSync(join(controlDir, 'config.md'), '---\nprovider: openai\n---\n', 'utf8');
  writeFileSync(join(controlDir, 'AGENTS.md'), 'agent rules\n', 'utf8');
  writeFileSync(join(controlDir, 'memory', 'memory.md'), 'remember this\n', 'utf8');

  const ws = createRunWorkspace({ baseDir, controlDir });

  assert.strictEqual(ws.mode, 'isolated-copy');
  assert.ok(existsSync(ws.runDir));
  assert.ok(existsSync(ws.repoDir));
  assert.ok(existsSync(ws.tasksDir));
  assert.ok(existsSync(join(ws.repoDir, '.git')));
  assert.strictEqual(readFileSync(join(ws.repoDir, 'AGENTS.md'), 'utf8'), 'agent rules\n');
  assert.strictEqual(readFileSync(join(ws.repoDir, 'memory', 'memory.md'), 'utf8'), 'remember this\n');

  const log = gitOk(['log', '-1', '--pretty=%s'], { cwd: ws.repoDir });
  assert.ok(log.stdout.includes('initialize execution snapshot'));
});
