import { Provider } from '../core/provider.js';
import { AnthropicProvider } from './anthropic.js';
import { OpenAIProvider } from './openai.js';
import { GeminiProvider } from './gemini.js';

export type ProviderName = 'anthropic' | 'openai' | 'gemini';

/**
 * Create a provider from environment variables.
 *
 * PICOAGENT_PROVIDER: anthropic | openai | gemini (default: anthropic)
 * PICOAGENT_MODEL: model name (defaults per provider)
 * PICOAGENT_MAX_TOKENS: max output tokens (default: 4096)
 *
 * Provider-specific:
 *   ANTHROPIC_API_KEY
 *   OPENAI_API_KEY + OPENAI_BASE_URL (optional, for compatible APIs)
 *   GEMINI_API_KEY
 */
export function createProvider(systemPrompt?: string): Provider {
  const providerName = (process.env.PICOAGENT_PROVIDER || 'anthropic') as ProviderName;
  const maxTokens = parseInt(process.env.PICOAGENT_MAX_TOKENS || '4096', 10);

  switch (providerName) {
    case 'anthropic': {
      const apiKey = process.env.ANTHROPIC_API_KEY;
      if (!apiKey) {
        console.error('Error: ANTHROPIC_API_KEY is required for anthropic provider');
        process.exit(1);
      }
      const model = process.env.PICOAGENT_MODEL || 'claude-sonnet-4-20250514';
      return new AnthropicProvider({ apiKey, model, maxTokens, systemPrompt });
    }

    case 'openai': {
      const apiKey = process.env.OPENAI_API_KEY;
      if (!apiKey) {
        console.error('Error: OPENAI_API_KEY is required for openai provider');
        process.exit(1);
      }
      const model = process.env.PICOAGENT_MODEL || 'gpt-4o';
      const baseURL = process.env.OPENAI_BASE_URL;
      return new OpenAIProvider({ apiKey, model, maxTokens, systemPrompt, baseURL });
    }

    case 'gemini': {
      const apiKey = process.env.GEMINI_API_KEY;
      if (!apiKey) {
        console.error('Error: GEMINI_API_KEY is required for gemini provider');
        process.exit(1);
      }
      const model = process.env.PICOAGENT_MODEL || 'gemini-2.5-flash';
      return new GeminiProvider({ apiKey, model, maxTokens, systemPrompt });
    }

    default:
      console.error(`Error: Unknown provider "${providerName}". Use: anthropic, openai, gemini`);
      process.exit(1);
  }
}
