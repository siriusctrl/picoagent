import { test, expect } from 'bun:test';
import { createRuntimeContext } from '../../src/runtime/index.ts';
import { joinPath } from '../../src/fs/path.ts';
import { ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

test('tool registry exposes the full runtime tool surface', async () => {
  const root = await makeTempDir('picoagent-config-');
  await ensureDir(joinPath(root, '.pico'));
  await writeTextFile(joinPath(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }\n');

  try {
    const runtime = createRuntimeContext(root);
    expect(runtime.registry.all().map((tool) => tool.name)).toEqual(['glob', 'grep', 'read', 'patch', 'cmd']);
  } finally {
    await removeDir(root);
  }
});
