import { Provider } from '../core/provider.ts';
import { PicoConfig, resolveApiKey } from '../config/config.ts';
import { AnthropicProvider } from './anthropic.ts';
import { OpenAIProvider } from './openai.ts';
import { GeminiProvider } from './gemini.ts';
import { EchoProvider } from './echo.ts';

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
