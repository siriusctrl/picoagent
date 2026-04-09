import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SessionModeId, Tool } from '../core/types.js';
import { DocMeta, scan } from './frontmatter.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const DEFAULTS_DIR = join(__dirname, '..', '..', 'defaults');

function readOptional(filePath: string): string | null {
  if (!existsSync(filePath)) {
    return null;
  }

  return readFileSync(filePath, 'utf8').trim();
}

function readOptionalMany(filePaths: string[]): string[] {
  return filePaths.map((filePath) => readOptional(filePath)).filter((value): value is string => value !== null);
}

function scanMerged(subdir: string, workspaceDir: string): DocMeta[] {
  const merged = new Map<string, DocMeta>();

  for (const root of [DEFAULTS_DIR, workspaceDir]) {
    try {
      for (const doc of scan(join(root, subdir))) {
        const name = typeof doc.frontmatter.name === 'string' ? doc.frontmatter.name : doc.path;
        merged.set(name, doc);
      }
    } catch {
      continue;
    }
  }

  return [...merged.values()];
}

function buildSummary(title: string, docs: DocMeta[]): string | null {
  const lines = docs
    .map((doc) => {
      const name = typeof doc.frontmatter.name === 'string' ? doc.frontmatter.name : null;
      const description = typeof doc.frontmatter.description === 'string' ? doc.frontmatter.description : null;
      return name && description ? `- ${name}: ${description}` : null;
    })
    .filter((line): line is string => line !== null);

  return lines.length > 0 ? `## ${title}\n${lines.join('\n')}` : null;
}

function buildModeContract(mode: SessionModeId): string {
  if (mode === 'ask') {
    return [
      '## Operating Mode',
      'You are in ask mode.',
      '- Inspect the workspace before answering.',
      '- Use read and search tools to ground your answer.',
      '- Do not claim to have changed files or run commands, because those tools are unavailable in this mode.',
    ].join('\n');
  }

  return [
    '## Operating Mode',
    'You are in exec mode.',
    '- Read first when the codebase is unclear.',
    '- Make concrete edits and run commands when the user asks for action.',
    '- Verify meaningful changes with the cheapest relevant command.',
  ].join('\n');
}

function buildToolSummary(tools: Tool[]): string {
  return ['## Available Tools', ...tools.map((tool) => `- ${tool.name}: ${tool.description}`)].join('\n');
}

export function buildSystemPrompt(controlDir: string, mode: SessionModeId, tools: Tool[]): string {
  const sections: string[] = [];
  sections.push(readOptional(join(controlDir, 'SOUL.md')) ?? 'You are a pragmatic coding agent.');

  for (const fileName of ['USER.md', 'AGENTS.md']) {
    const content = readOptional(join(controlDir, fileName));
    if (content) {
      sections.push(content);
    }
  }

  const memories = readOptionalMany([
    join(homedir(), '.pico', 'memory', 'memory.md'),
    join(controlDir, '.pico', 'memory', 'memory.md'),
  ]);
  if (memories.length > 0) {
    sections.push(`## Core Memory\n${memories.join('\n\n')}`);
  }

  const skills = buildSummary('Available Skills', scanMerged('skills', controlDir));
  if (skills) {
    sections.push(skills);
  }

  const agents = buildSummary('Available Agents', scanMerged('agents', controlDir));
  if (agents) {
    sections.push(agents);
  }

  sections.push(buildModeContract(mode));
  sections.push(buildToolSummary(tools));

  return sections.join('\n\n');
}
