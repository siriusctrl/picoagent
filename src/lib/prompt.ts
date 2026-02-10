import { readFileSync, existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { scan, DocMeta } from './frontmatter.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const DEFAULTS_DIR = join(__dirname, '..', '..', 'defaults');

function readOptional(path: string): string | null {
  if (!existsSync(path)) return null;
  return readFileSync(path, 'utf-8').trim();
}

/**
 * Scan a directory from both defaults and workspace, workspace overrides by name.
 */
function scanMerged(subdir: string, workspaceDir: string): DocMeta[] {
  const byName = new Map<string, DocMeta>();

  // Scan defaults first
  const defaultsPath = join(DEFAULTS_DIR, subdir);
  try {
    for (const doc of scan(defaultsPath)) {
      const name = doc.frontmatter.name as string;
      if (name) byName.set(name, doc);
    }
  } catch { /* defaults dir may not exist */ }

  // Workspace overrides by name
  const workspacePath = join(workspaceDir, subdir);
  try {
    for (const doc of scan(workspacePath)) {
      const name = doc.frontmatter.name as string;
      if (name) byName.set(name, doc);
    }
  } catch { /* workspace dir may not exist */ }

  return [...byName.values()];
}

function buildSummary(title: string, docs: DocMeta[]): string {
  const lines = docs
    .map((d) => {
      const name = d.frontmatter.name as string | undefined;
      const desc = d.frontmatter.description as string | undefined;
      return name && desc ? `- ${name}: ${desc}` : null;
    })
    .filter(Boolean);
  if (lines.length === 0) return '';
  return `## ${title}\n${lines.join('\n')}`;
}

/**
 * Build the system prompt for the Main Agent.
 *
 * Assembly order:
 *   SOUL.md → USER.md → AGENTS.md → memory.md → skill summaries → agent summaries
 *
 * Tools are provided via the provider's structured tool interface, not the prompt.
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

  const skills = scanMerged('skills', workspaceDir);
  const skillSummary = buildSummary('Available Skills', skills);
  if (skillSummary) sections.push(skillSummary);

  const agentProfiles = scanMerged('agents', workspaceDir);
  const agentSummary = buildSummary('Available Agents', agentProfiles);
  if (agentSummary) sections.push(agentSummary);

  return sections.join('\n\n');
}

/**
 * Build the system prompt for a Worker.
 *
 * Assembly order:
 *   AGENTS.md → skill summaries → protocol → constraints → task instructions (last)
 *
 * Tools are provided via the provider's structured tool interface, not the prompt.
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

  const skills = scanMerged('skills', workspaceDir);
  const skillSummary = buildSummary('Available Skills', skills);
  if (skillSummary) sections.push(skillSummary);

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
