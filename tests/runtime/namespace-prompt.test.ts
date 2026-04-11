import { expect, test } from 'bun:test';
import { buildNamespacePromptSection } from '../../src/runtime/namespace-prompt.ts';

test('buildNamespacePromptSection describes namespace capabilities and explicit cmd cwd', () => {
  const section = buildNamespacePromptSection([
    { name: 'workspace', writable: true, supportsCmd: true },
    { name: 'sandbox', writable: true, supportsCmd: true },
    { name: 'session', writable: false, supportsCmd: false },
  ]);

  expect(section).toContain('## File Views');
  expect(section).toContain('/workspace: main read/write workspace, cmd enabled');
  expect(section).toContain('/sandbox: mounted read/write file-view, cmd enabled');
  expect(section).toContain('/session: read-only session history, cmd disabled');
  expect(section).toContain('cmd always requires an explicit cwd path like /workspace or /sandbox/tmp.');
});
