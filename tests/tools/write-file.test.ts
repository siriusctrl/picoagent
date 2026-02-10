import { test } from 'node:test';
import assert from 'node:assert/strict';
import { writeFileTool } from '../../src/tools/write-file.js';
import { ToolContext } from '../../src/core/types.js';
import { join } from 'path';
import { tmpdir } from 'os';
import { mkdtempSync, readFileSync, rmSync } from 'fs';

test('write_file respects writeRoot boundary', async () => {
  const tmpDir = mkdtempSync(join(tmpdir(), 'picoagent-write-'));
  const allowedDir = join(tmpDir, 'allowed');
  
  const context: ToolContext = {
    cwd: tmpDir,
    tasksRoot: join(tmpDir, '.tasks'),
    writeRoot: allowedDir,
  };

  // Should succeed: writing inside writeRoot
  const ok = await writeFileTool.execute(
    { path: join(allowedDir, 'test.txt'), content: 'hello' },
    context
  );
  assert.ok(!ok.isError, `Expected success but got: ${ok.content}`);
  assert.strictEqual(readFileSync(join(allowedDir, 'test.txt'), 'utf-8'), 'hello');

  // Should fail: writing outside writeRoot
  const denied = await writeFileTool.execute(
    { path: join(tmpDir, 'escape.txt'), content: 'nope' },
    context
  );
  assert.ok(denied.isError);
  assert.ok(denied.content.includes('Write denied'));

  rmSync(tmpDir, { recursive: true, force: true });
});

test('write_file allows all writes when writeRoot not set', async () => {
  const tmpDir = mkdtempSync(join(tmpdir(), 'picoagent-write-'));

  const context: ToolContext = {
    cwd: tmpDir,
    tasksRoot: join(tmpDir, '.tasks'),
    // no writeRoot
  };

  const ok = await writeFileTool.execute(
    { path: join(tmpDir, 'anywhere.txt'), content: 'hello' },
    context
  );
  assert.ok(!ok.isError);

  rmSync(tmpDir, { recursive: true, force: true });
});
