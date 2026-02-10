import { readFileSync, existsSync } from 'fs';
import { join } from 'path';
import { scan } from './frontmatter.js';

function readOptional(path: string): string | null {
  if (!existsSync(path)) return null;
  return readFileSync(path, 'utf-8').trim();
}

function buildSkillSummary(skillsDir: string): string {
  try {
    const skills = scan(skillsDir);
    if (skills.length === 0) return '';
    const lines = skills
      .map((s) => {
        const name = s.frontmatter.name as string | undefined;
        const desc = s.frontmatter.description as string | undefined;
        return name && desc ? `- ${name}: ${desc}` : null;
      })
      .filter(Boolean);
    if (lines.length === 0) return '';
    return `## Available Skills\n${lines.join('\n')}`;
  } catch {
    return '';
  }
}

/**
 * Build the system prompt for the Main Agent.
 *
 * Assembly order:
 *   SOUL.md → USER.md → AGENTS.md → memory.md → skill summaries → tool hints
 */
export function buildMainPrompt(workspaceDir: string): string {
  const sections: string[] = [];

  const soul = readOptional(join(workspaceDir, 'SOUL.md'));
  if (soul) sections.push(soul);
  else sections.push('You are a helpful coding assistant.');

  const user = readOptional(join(workspaceDir, 'USER.md'));
  if (user) sections.push(user);

  const agents = readOptional(join(workspaceDir, 'AGENTS.md'));
  if (agents) sections.push(agents);

  const memory = readOptional(join(workspaceDir, 'memory', 'memory.md'));
  if (memory) sections.push(`## Core Memory\n${memory}`);

  const skills = buildSkillSummary(join(workspaceDir, 'skills'));
  if (skills) sections.push(skills);

  sections.push(
    `## Tools\n` +
    `Use scan(dir, pattern?) to search markdown files by frontmatter.\n` +
    `Use load(path) to read full file content.\n` +
    `Use scan("memory/") to search through memories.\n` +
    `Use dispatch(task) to send research/analysis tasks to background workers.`
  );

  return sections.join('\n\n');
}

/**
 * Build the system prompt for a Worker.
 *
 * Assembly order:
 *   AGENTS.md → skill summaries → tool hints → protocol → constraints → task instructions (last)
 */
export function buildWorkerPrompt(
  taskDir: string,
  workspaceDir: string,
  taskBody: string,
  taskId: string,
  taskName: string,
  taskDesc: string
): string {
  const sections: string[] = [];

  const agents = readOptional(join(workspaceDir, 'AGENTS.md'));
  if (agents) sections.push(agents);

  const skills = buildSkillSummary(join(workspaceDir, 'skills'));
  if (skills) sections.push(skills);

  sections.push(
    `## Tools\n` +
    `Use scan(dir, pattern?) to search markdown files by frontmatter.\n` +
    `Use load(path) to read full file content.\n` +
    `Use shell(command) to run commands.\n` +
    `Use read_file(path) to read any file.\n` +
    `Use write_file(path, content) to write files (restricted to your working directory).`
  );

  sections.push(
    `## Protocol\n` +
    `1. Update progress.md with your current status and plan.\n` +
    `2. Write the final result to result.md.\n` +
    `3. If you fail, write the error to result.md.`
  );

  sections.push(
    `## Working Directory\n` +
    `Your working directory is: ${taskDir}\n` +
    `All file outputs must be written here.\n` +
    `The project workspace at ${workspaceDir} is available for reading and reference only.`
  );

  sections.push(
    `## Task: ${taskId} — ${taskName}\n` +
    `${taskDesc}\n\n` +
    `### Instructions\n${taskBody}`
  );

  return sections.join('\n\n');
}
