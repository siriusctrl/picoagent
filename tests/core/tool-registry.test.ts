import { test, expect } from 'bun:test';
import { createRuntimeContext } from '../../src/runtime/index.ts';
import { joinPath } from '../../src/fs/path.ts';
import { ensureDir, makeTempDir, removeDir, writeTextFile } from '../helpers/fs.ts';

test('tool registry equips ask and exec agent presets with different tool subsets', async () => {
  const root = await makeTempDir('picoagent-config-');
  await ensureDir(joinPath(root, '.pico'));
  await writeTextFile(joinPath(root, '.pico', 'config.jsonc'), '{ "provider": "echo", "model": "echo" }\n');

  try {
    const runtime = createRuntimeContext(root);
    const askTools = runtime.registry.forAgent('ask').map((tool) => tool.name);
    const execTools = runtime.registry.forAgent('exec').map((tool) => tool.name);

    expect(askTools).toEqual(['glob', 'grep', 'read']);
    expect(execTools).toEqual(['glob', 'grep', 'read', 'patch', 'cmd']);
  } finally {
    await removeDir(root);
  }
});
