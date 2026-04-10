import { test } from 'node:test';
import assert from 'node:assert/strict';
import { buildSystemPrompt } from '../../src/prompting/prompt.js';
import { grepTool } from '../../src/tools/grep.js';

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
      agentsDocs: [
        { path: 'defaults/agents/exec.md', frontmatter: { name: 'exec', description: 'run changes' } },
      ],
    },
    'ask',
    [grepTool],
  );

  assert.match(prompt, /workspace soul/);
  assert.match(prompt, /workspace user/);
  assert.match(prompt, /workspace agents/);
  assert.match(prompt, /user memory/);
  assert.match(prompt, /workspace memory/);
  assert.match(prompt, /Available Skills/);
  assert.match(prompt, /readme: read docs/);
  assert.match(prompt, /Available Agents/);
  assert.match(prompt, /exec: run changes/);
});
