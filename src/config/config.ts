import { joinPath } from '../fs/path.ts';

export interface PicoConfig {
  provider: 'anthropic' | 'openai' | 'gemini' | 'echo';
  model: string;
  maxTokens: number;
  contextWindow: number;
  baseURL?: string;
}

const PICO_DIR = '.pico';
const CONFIG_FILE = 'config.jsonc';

const DEFAULTS: Record<PicoConfig['provider'], Partial<PicoConfig>> = {
  anthropic: { model: 'claude-sonnet-4-20250514' },
  openai: { model: 'gpt-4o' },
  gemini: { model: 'gemini-2.5-flash' },
  echo: { model: 'echo' },
};

function defaultConfig(): PicoConfig {
  return {
    provider: 'echo',
    model: DEFAULTS.echo.model ?? 'echo',
    maxTokens: 4096,
    contextWindow: 200000,
    baseURL: undefined,
  };
}

function stripJsonComments(input: string): string {
  let output = '';
  let inString = false;
  let escaped = false;
  let lineComment = false;
  let blockComment = false;

  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];
    const next = input[index + 1];

    if (lineComment) {
      if (char === '\n') {
        lineComment = false;
        output += char;
      }
      continue;
    }

    if (blockComment) {
      if (char === '*' && next === '/') {
        blockComment = false;
        index += 1;
      } else if (char === '\n') {
        output += char;
      }
      continue;
    }

    if (inString) {
      output += char;
      if (escaped) {
        escaped = false;
      } else if (char === '\\') {
        escaped = true;
      } else if (char === '"') {
        inString = false;
      }
      continue;
    }

    if (char === '/' && next === '/') {
      lineComment = true;
      index += 1;
      continue;
    }

    if (char === '/' && next === '*') {
      blockComment = true;
      index += 1;
      continue;
    }

    if (char === '"') {
      inString = true;
    }

    output += char;
  }

  return output;
}

function stripTrailingCommas(input: string): string {
  let output = '';
  let inString = false;
  let escaped = false;

  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];

    if (inString) {
      output += char;
      if (escaped) {
        escaped = false;
      } else if (char === '\\') {
        escaped = true;
      } else if (char === '"') {
        inString = false;
      }
      continue;
    }

    if (char === '"') {
      inString = true;
      output += char;
      continue;
    }

    if (char === ',') {
      let lookahead = index + 1;
      while (lookahead < input.length && /\s/.test(input[lookahead])) {
        lookahead += 1;
      }

      if (input[lookahead] === '}' || input[lookahead] === ']') {
        continue;
      }
    }

    output += char;
  }

  return output;
}

function parseJsonc(raw: string, filePath: string): Record<string, unknown> {
  try {
    return JSON.parse(stripTrailingCommas(stripJsonComments(raw))) as Record<string, unknown>;
  } catch (error: unknown) {
    throw new Error(
      `Invalid JSONC in ${filePath}: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

async function readConfigFile(filePath: string): Promise<Record<string, unknown> | null> {
  const file = Bun.file(filePath);
  if (!(await file.exists())) {
    return null;
  }

  return parseJsonc(await file.text(), filePath);
}

function configPath(root: string): string {
  return joinPath(root, PICO_DIR, CONFIG_FILE);
}

export function userHomeDir(): string {
  return process.env.HOME || Bun.env.HOME || '/';
}

export function workspaceConfigPath(workspaceDir: string): string {
  return configPath(workspaceDir);
}

export function userConfigPath(): string {
  return configPath(userHomeDir());
}

async function mergedFrontmatter(workspaceDir: string): Promise<Record<string, unknown>> {
  return {
    ...await readConfigFile(userConfigPath()),
    ...await readConfigFile(workspaceConfigPath(workspaceDir)),
  };
}

function resolveConfig(workspaceDir: string, raw: Record<string, unknown>): PicoConfig {
  const providerValue = typeof raw.provider === 'string' ? raw.provider : defaultConfig().provider;

  if (!['anthropic', 'openai', 'gemini', 'echo'].includes(providerValue)) {
    throw new Error(
      `invalid provider "${providerValue}" in ${workspaceConfigPath(workspaceDir)} or ${userConfigPath()}. ` +
        'Use: anthropic, openai, gemini, echo',
    );
  }

  const provider = providerValue as PicoConfig['provider'];
  const providerDefaults = DEFAULTS[provider];

  return {
    provider,
    model: typeof raw.model === 'string' ? raw.model : providerDefaults.model ?? defaultConfig().model,
    maxTokens: typeof raw.maxTokens === 'number' ? raw.maxTokens : defaultConfig().maxTokens,
    contextWindow: typeof raw.contextWindow === 'number' ? raw.contextWindow : defaultConfig().contextWindow,
    baseURL: typeof raw.baseURL === 'string' ? raw.baseURL : undefined,
  };
}

export function loadConfigFromContents(
  workspaceDir: string,
  sources: { userConfig?: string | null; workspaceConfig?: string | null },
): PicoConfig {
  const raw = {
    ...(sources.userConfig ? parseJsonc(sources.userConfig, userConfigPath()) : {}),
    ...(sources.workspaceConfig ? parseJsonc(sources.workspaceConfig, workspaceConfigPath(workspaceDir)) : {}),
  };

  return resolveConfig(workspaceDir, raw);
}

/**
 * Load config by shallow-merging `$HOME/.pico/config.jsonc` with
 * `<workspace>/.pico/config.jsonc`. Workspace fields override user fields.
 * If neither file exists, use built-in defaults.
 */
export async function loadConfig(workspaceDir: string): Promise<PicoConfig> {
  return resolveConfig(workspaceDir, await mergedFrontmatter(workspaceDir));
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

  if (provider === 'echo') {
    return '';
  }

  const envVar = envMap[provider];
  if (!envVar) {
    throw new Error(`unknown provider "${provider}"`);
  }

  const key = process.env[envVar];
  if (!key) {
    throw new Error(`${envVar} environment variable is required for ${provider} provider`);
  }

  return key;
}
