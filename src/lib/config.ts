import { existsSync, readFileSync } from 'fs';
import { join } from 'path';
import { parseFrontmatter } from './frontmatter.js';

export interface PicoConfig {
  provider: 'anthropic' | 'openai' | 'gemini';
  model: string;
  maxTokens: number;
  contextWindow: number;
  baseURL?: string;  // for OpenAI-compatible APIs
}

const DEFAULTS: Record<string, Partial<PicoConfig>> = {
  anthropic: { model: 'claude-sonnet-4-20250514' },
  openai: { model: 'gpt-4o' },
  gemini: { model: 'gemini-2.5-flash' },
};

/**
 * Load config from workspace/config.md.
 * Frontmatter fields: provider, model, max_tokens, context_window, base_url
 */
export function loadConfig(workspaceDir: string): PicoConfig {
  const configPath = join(workspaceDir, 'config.md');

  if (!existsSync(configPath)) {
    console.error(`Error: config.md not found in ${workspaceDir}`);
    console.error('Create a config.md with at least:\n---\nprovider: anthropic\n---');
    process.exit(1);
  }

  const raw = readFileSync(configPath, 'utf-8');
  const { frontmatter } = parseFrontmatter(raw);

  const provider = String(frontmatter.provider || '');
  if (!['anthropic', 'openai', 'gemini'].includes(provider)) {
    console.error(`Error: invalid provider "${provider}" in config.md. Use: anthropic, openai, gemini`);
    process.exit(1);
  }

  const defaults = DEFAULTS[provider];

  return {
    provider: provider as PicoConfig['provider'],
    model: String(frontmatter.model || defaults?.model || ''),
    maxTokens: Number(frontmatter.max_tokens || 4096),
    contextWindow: Number(frontmatter.context_window || 200000),
    baseURL: frontmatter.base_url ? String(frontmatter.base_url) : undefined,
  };
}

/**
 * Resolve the API key for a provider from environment variables.
 */
export function resolveApiKey(provider: string): string {
  const envMap: Record<string, string> = {
    anthropic: 'ANTHROPIC_API_KEY',
    openai: 'OPENAI_API_KEY',
    gemini: 'GEMINI_API_KEY',
  };

  const envVar = envMap[provider];
  if (!envVar) {
    console.error(`Error: unknown provider "${provider}"`);
    process.exit(1);
  }

  const key = process.env[envVar];
  if (!key) {
    console.error(`Error: ${envVar} environment variable is required for ${provider} provider`);
    process.exit(1);
  }

  return key;
}
