import { AgentPresetId, Tool } from '../core/types.js';
import { DocMeta } from './frontmatter.js';

export interface ControlPromptSurface {
  soul: string | null;
  user: string | null;
  agents: string | null;
  memories: string[];
  skills: DocMeta[];
  agentsDocs: DocMeta[];
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

function buildAgentContract(agent: AgentPresetId): string {
  if (agent === 'ask') {
    return [
      '## Active Agent',
      'You are running the ask agent preset.',
      '- Inspect the workspace before answering.',
      '- Use read and search tools to ground your answer.',
      '- Do not claim to have changed files or run commands, because those tools are unavailable in this preset.',
    ].join('\n');
  }

  return [
    '## Active Agent',
    'You are running the exec agent preset.',
    '- Read first when the codebase is unclear.',
    '- Make concrete edits and run commands when the user asks for action.',
    '- Verify meaningful changes with the cheapest relevant command.',
  ].join('\n');
}

function buildToolSummary(tools: Tool[]): string {
  return ['## Available Tools', ...tools.map((tool) => `- ${tool.name}: ${tool.description}`)].join('\n');
}

export function buildSystemPrompt(surface: ControlPromptSurface, agent: AgentPresetId, tools: Tool[]): string {
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

  const agents = buildSummary('Available Agents', surface.agentsDocs);
  if (agents) {
    sections.push(agents);
  }

  sections.push(buildAgentContract(agent));
  sections.push(buildToolSummary(tools));

  return sections.join('\n\n');
}
