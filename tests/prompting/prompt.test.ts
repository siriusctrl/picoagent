import { expect, test } from 'bun:test';
import { buildSystemPrompt } from '../../src/prompting/prompt.ts';
import { grepTool } from '../../src/tools/grep.ts';

test('buildSystemPrompt formats a preloaded control surface', () => {
  const prompt = buildSystemPrompt(
    {
      soul: 'workspace soul',
      user: 'workspace user',
      agents: 'workspace agents',
      memories: ['user memory', 'workspace memory'],
      skills: [
        { path: 'defaults/skills/readme.md', frontmatter: { name: 'readme', description: 'read docs' } },
      ],
    },
    [grepTool],
  );

  expect(prompt).toMatch(/workspace soul/);
  expect(prompt).toMatch(/workspace user/);
  expect(prompt).toMatch(/workspace agents/);
  expect(prompt).toMatch(/user memory/);
  expect(prompt).toMatch(/workspace memory/);
  expect(prompt).toMatch(/Available Skills/);
  expect(prompt).toMatch(/readme: read docs/);
});
