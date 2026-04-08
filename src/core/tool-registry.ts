import { SessionModeId, Tool } from './types.js';

export interface ToolRegistryConfig {
  tools: Tool[];
  modeTools: Record<SessionModeId, string[]>;
}

export class ToolRegistry {
  private readonly toolsByName: Map<string, Tool>;
  private readonly modeTools: Record<SessionModeId, string[]>;

  constructor(config: ToolRegistryConfig) {
    this.toolsByName = new Map(config.tools.map((tool) => [tool.name, tool]));
    this.modeTools = config.modeTools;
  }

  all(): Tool[] {
    return [...this.toolsByName.values()];
  }

  get(name: string): Tool | undefined {
    return this.toolsByName.get(name);
  }

  forMode(mode: SessionModeId): Tool[] {
    return this.modeTools[mode]
      .map((name) => this.toolsByName.get(name))
      .filter((tool): tool is Tool => tool !== undefined);
  }
}
