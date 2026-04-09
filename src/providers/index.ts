import { Provider } from '../core/provider.js';
import { PicoConfig, resolveApiKey } from '../config/config.js';
import { AnthropicProvider } from './anthropic.js';
import { OpenAIProvider } from './openai.js';
import { GeminiProvider } from './gemini.js';
import { EchoProvider } from './echo.js';

/**
 * Create a provider from PicoConfig.
 * API keys are resolved from environment variables.
 */
export function createProvider(config: PicoConfig, systemPrompt?: string): Provider {
  if (config.provider === 'echo') {
    return new EchoProvider({
      apiKey: '',
      model: config.model,
      maxTokens: config.maxTokens,
      systemPrompt,
    });
  }

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
