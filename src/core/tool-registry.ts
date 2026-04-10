import { AgentPresetId, Tool } from './types.ts';

export interface ToolRegistryConfig {
  tools: Tool[];
  agentTools: Record<AgentPresetId, string[]>;
}

export class ToolRegistry {
  private readonly toolsByName: Map<string, Tool>;
  private readonly agentTools: Record<AgentPresetId, string[]>;

  constructor(config: ToolRegistryConfig) {
    this.toolsByName = new Map(config.tools.map((tool) => [tool.name, tool]));
    this.agentTools = config.agentTools;
  }

  all(): Tool[] {
    return [...this.toolsByName.values()];
  }

  get(name: string): Tool | undefined {
    return this.toolsByName.get(name);
  }

  forAgent(agent: AgentPresetId): Tool[] {
    return this.agentTools[agent]
      .map((name) => this.toolsByName.get(name))
      .filter((tool): tool is Tool => tool !== undefined);
  }
}
