import { test } from 'node:test';
import assert from 'node:assert';
import { mkdtempSync, writeFileSync, readFileSync, existsSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import { runSandboxedShell } from '../../src/lib/sandbox.js';

function hasBwrap(): boolean {
  return existsSync('/usr/bin/bwrap');
}

test('sandbox: allows writing inside writeRoot', { skip: !hasBwrap() }, async () => {
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

test('sandbox: prevents writing outside writeRoot (e.g. /etc)', { skip: !hasBwrap() }, async () => {
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
