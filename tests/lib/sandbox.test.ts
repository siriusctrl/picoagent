import { test } from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, writeFileSync, readFileSync, existsSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import { spawnSync } from 'child_process';
import { runSandboxedShell } from '../../src/lib/sandbox.js';

function canUseBwrap(): boolean {
  if (!existsSync('/usr/bin/bwrap')) return false;
  // Preflight: some CI environments disable user namespaces.
  const res = spawnSync('/usr/bin/bwrap', ['--unshare-user', '--ro-bind', '/', '/', 'true'], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  return (res.status ?? 1) === 0;
}

const BWRAP_OK = canUseBwrap();
if (!BWRAP_OK) {
  // Visible warning in CI logs.
  console.warn('[warn] bubblewrap sandbox not available (missing bwrap or userns disabled); skipping sandbox integration assertions');
}

test('sandbox: allows writing inside writeRoot', { skip: !BWRAP_OK }, async () => {
  const dir = mkdtempSync(join(tmpdir(), 'pico-sb-'));
  const file = join(dir, 'a.txt');
  writeFileSync(file, 'hello\n', 'utf8');

  const res = await runSandboxedShell({
    enabled: true,
    writeRoot: dir,
    cwd: dir,
    command: 'echo world >> a.txt',
    timeoutMs: 10_000,
  });

  assert.strictEqual(res.timedOut, false);
  assert.strictEqual(res.code, 0);
  const content = readFileSync(file, 'utf8');
  assert.ok(content.includes('world'));
});

test('sandbox: prevents writing outside writeRoot (e.g. /etc)', { skip: !BWRAP_OK }, async () => {
  const dir = mkdtempSync(join(tmpdir(), 'pico-sb-'));

  const res = await runSandboxedShell({
    enabled: true,
    writeRoot: dir,
    cwd: dir,
    command: 'echo nope >> /etc/picoagent_should_fail',
    timeoutMs: 10_000,
  });

  assert.notStrictEqual(res.code, 0);
  assert.ok(res.stderr.includes('Read-only file system') || res.stdout.includes('Read-only file system'));
});

test('sandbox: rejects cwd outside writeRoot', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pico-sb-root-'));
  const other = mkdtempSync(join(tmpdir(), 'pico-sb-other-'));

  await assert.rejects(
    () => runSandboxedShell({ enabled: true, writeRoot: root, cwd: other, command: 'echo hi' }),
    /cwd is outside writeRoot/
  );
});
