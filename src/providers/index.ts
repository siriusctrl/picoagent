import { Provider } from '../core/provider.js';
import { PicoConfig, resolveApiKey } from '../lib/config.js';
import { AnthropicProvider } from './anthropic.js';
import { OpenAIProvider } from './openai.js';
import { GeminiProvider } from './gemini.js';

/**
 * Create a provider from PicoConfig.
 * API keys are resolved from environment variables.
 */
export function createProvider(config: PicoConfig, systemPrompt?: string): Provider {
  const apiKey = resolveApiKey(config.provider);

  switch (config.provider) {
    case 'anthropic':
      return new AnthropicProvider({
        apiKey,
        model: config.model,
        maxTokens: config.maxTokens,
        systemPrompt,
      });

    case 'openai':
      return new OpenAIProvider({
        apiKey,
        model: config.model,
        maxTokens: config.maxTokens,
        systemPrompt,
        baseURL: config.baseURL,
      });

    case 'gemini':
      return new GeminiProvider({
        apiKey,
        model: config.model,
        maxTokens: config.maxTokens,
        systemPrompt,
      });
  }
}
