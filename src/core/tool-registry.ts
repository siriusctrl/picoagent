import { Tool } from './types.ts';

export interface ToolRegistryConfig {
  tools: Tool[];
}

export class ToolRegistry {
  private readonly toolsByName: Map<string, Tool>;

  constructor(config: ToolRegistryConfig) {
    this.toolsByName = new Map(config.tools.map((tool) => [tool.name, tool]));
  }

  all(): Tool[] {
    return [...this.toolsByName.values()];
  }

  get(name: string): Tool | undefined {
    return this.toolsByName.get(name);
  }
}
