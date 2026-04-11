import { Tool } from '../core/types.ts';
import { DocMeta } from './frontmatter.ts';

export interface ControlPromptSurface {
  soul: string | null;
  user: string | null;
  agents: string | null;
  memories: string[];
  skills: DocMeta[];
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

function buildToolSummary(tools: Tool[]): string {
  return ['## Available Tools', ...tools.map((tool) => `- ${tool.name}: ${tool.description}`)].join('\n');
}

export function buildSystemPrompt(surface: ControlPromptSurface, tools: Tool[]): string {
  const sections: string[] = [];
  sections.push(surface.soul ?? 'You are a pragmatic coding agent.');

  for (const content of [surface.user, surface.agents]) {
    if (content) {
      sections.push(content);
    }
  }

  if (surface.memories.length > 0) {
    sections.push(`## Core Memory\n${surface.memories.join('\n\n')}`);
  }

  const skills = buildSummary('Available Skills', surface.skills);
  if (skills) {
    sections.push(skills);
  }

  sections.push(buildToolSummary(tools));

  return sections.join('\n\n');
}
