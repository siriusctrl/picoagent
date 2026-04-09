import * as acp from '@agentclientprotocol/sdk';
import { randomUUID } from 'node:crypto';
import { createAppBootstrap } from '../bootstrap/index.js';
import { AcpEnvironment } from './environment.js';
import { runAgentLoop } from '../core/loop.js';
import { Message, SessionModeId, ToolOutput } from '../core/types.js';
import { buildSystemPrompt } from '../prompting/prompt.js';

interface SessionState {
  id: string;
  cwd: string;
  roots: string[];
  mode: SessionModeId;
  messages: Message[];
  controller?: AbortController;
}

const MODES: acp.SessionMode[] = [
  {
    id: 'ask',
    name: 'Ask',
    description: 'Inspect, search, explain, and plan without modifying the workspace.',
  },
  {
    id: 'exec',
    name: 'Exec',
    description: 'Inspect, edit files, and run commands directly in the workspace.',
  },
];

function promptToText(blocks: acp.ContentBlock[]): string {
  return blocks
    .map((block) => {
      if (block.type === 'text') {
        return block.text;
      }

      if (block.type === 'resource_link') {
        return `[resource: ${block.uri}]`;
      }

      if (block.type === 'resource') {
        return `[resource: ${block.resource.uri}]`;
      }

      return `[${block.type}]`;
    })
    .join('\n');
}

function toAcpContent(content?: ToolOutput[]): acp.ToolCallContent[] | undefined {
  if (!content || content.length === 0) {
    return undefined;
  }

  return content.map((item) => {
    switch (item.type) {
      case 'text':
        return {
          type: 'content',
          content: {
            type: 'text',
            text: item.text,
          },
        };
      case 'diff':
        return {
          type: 'diff',
          path: item.path,
          oldText: item.oldText,
          newText: item.newText,
        };
      case 'terminal':
        return {
          type: 'terminal',
          terminalId: item.terminalId,
        };
    }
  });
}

function fallbackToolTitle(tool: import('../core/types.js').Tool | undefined, callName: string): string {
  if (!tool) {
    return 'Unknown tool';
  }

  return typeof tool.title === 'string' ? tool.title : tool.name ?? callName;
}

export class PicoAgent {
  private readonly bootstrap = createAppBootstrap(process.cwd());
  private readonly sessions = new Map<string, SessionState>();
  private readonly environment: AcpEnvironment;

  constructor(private readonly connection: acp.AgentSideConnection) {
    this.environment = new AcpEnvironment(connection);
  }

  async initialize(): Promise<acp.InitializeResponse> {
    return {
      protocolVersion: acp.PROTOCOL_VERSION,
      agentCapabilities: {
        promptCapabilities: {},
        sessionCapabilities: {},
      },
    };
  }

  async authenticate(): Promise<acp.AuthenticateResponse> {
    return {};
  }

  async newSession(params: acp.NewSessionRequest): Promise<acp.NewSessionResponse> {
    const sessionId = randomUUID();
    this.sessions.set(sessionId, {
      id: sessionId,
      cwd: params.cwd,
      roots: [params.cwd, ...(params.additionalDirectories ?? [])],
      mode: 'ask',
      messages: [],
    });

    return {
      sessionId,
      modes: {
        availableModes: MODES,
        currentModeId: 'ask',
      },
    };
  }

  async setSessionMode(params: acp.SetSessionModeRequest): Promise<acp.SetSessionModeResponse> {
    const session = this.requireSession(params.sessionId);
    if (params.modeId !== 'ask' && params.modeId !== 'exec') {
      throw new Error(`Unsupported mode: ${params.modeId}`);
    }

    session.mode = params.modeId;
    await this.connection.sessionUpdate({
      sessionId: session.id,
      update: {
        sessionUpdate: 'current_mode_update',
        currentModeId: session.mode,
      },
    });

    return {};
  }

  async prompt(params: acp.PromptRequest): Promise<acp.PromptResponse> {
    const session = this.requireSession(params.sessionId);
    session.controller?.abort();

    const controller = new AbortController();
    session.controller = controller;
    const promptText = promptToText(params.prompt);
    session.messages.push({ role: 'user', content: promptText });

    const tools = this.bootstrap.registry.forMode(session.mode);
    const systemPrompt = buildSystemPrompt(this.bootstrap.controlDir, session.mode, tools);

    try {
      await runAgentLoop(
        session.messages,
        tools,
        this.bootstrap.provider,
        {
          sessionId: session.id,
          cwd: session.cwd,
          roots: session.roots,
          controlRoot: this.bootstrap.controlDir,
          mode: session.mode,
          signal: controller.signal,
          environment: this.environment,
        },
        systemPrompt,
        {
          onTextDelta: async (text) => {
            await this.connection.sessionUpdate({
              sessionId: session.id,
              update: {
                sessionUpdate: 'agent_message_chunk',
                content: {
                  type: 'text',
                  text,
                },
              },
            });
          },
          onToolStart: async (call, tool) => {
            const parsedArgs = tool?.parameters.safeParse(call.arguments);
            const resolved =
              parsedArgs && parsedArgs.success
                ? (() => {
                    try {
                      return {
                        title:
                          tool && typeof tool.title === 'function'
                            ? tool.title(parsedArgs.data, {
                                sessionId: session.id,
                                cwd: session.cwd,
                                roots: session.roots,
                                controlRoot: this.bootstrap.controlDir,
                                mode: session.mode,
                                signal: controller.signal,
                                environment: this.environment,
                              })
                            : fallbackToolTitle(tool, call.name),
                        locations: tool?.locations
                          ? tool.locations(parsedArgs.data, {
                              sessionId: session.id,
                              cwd: session.cwd,
                              roots: session.roots,
                              controlRoot: this.bootstrap.controlDir,
                              mode: session.mode,
                              signal: controller.signal,
                              environment: this.environment,
                            })
                          : [],
                      };
                    } catch {
                      return {
                        title: fallbackToolTitle(tool, call.name),
                        locations: [],
                      };
                    }
                  })()
                : {
                    title: fallbackToolTitle(tool, call.name),
                    locations: [],
                  };

            await this.connection.sessionUpdate({
              sessionId: session.id,
              update: {
                sessionUpdate: 'tool_call',
                toolCallId: call.id,
                title: resolved.title,
                kind: tool?.kind ?? 'other',
                status: 'pending',
                locations: resolved.locations,
                rawInput: call.arguments,
              },
            });
          },
          onToolEnd: async (call, _tool, result) => {
            await this.connection.sessionUpdate({
              sessionId: session.id,
              update: {
                sessionUpdate: 'tool_call_update',
                toolCallId: call.id,
                title: result.title,
                kind: result.kind,
                status: result.message.isError ? 'failed' : 'completed',
                locations: result.locations,
                content:
                  toAcpContent(result.display) ??
                  [
                    {
                      type: 'content',
                      content: {
                        type: 'text',
                        text: result.message.content,
                      },
                    },
                  ],
                rawOutput: result.rawOutput ?? { content: result.message.content },
              },
            });
          },
        },
      );

      return {
        stopReason: 'end_turn',
        userMessageId: params.messageId ?? undefined,
      };
    } catch (error) {
      if (controller.signal.aborted || this.connection.signal.aborted) {
        return {
          stopReason: 'cancelled',
          userMessageId: params.messageId ?? undefined,
        };
      }

      throw error;
    } finally {
      if (session.controller === controller) {
        session.controller = undefined;
      }
    }
  }

  async cancel(params: acp.CancelNotification): Promise<void> {
    this.sessions.get(params.sessionId)?.controller?.abort();
  }

  private requireSession(sessionId: string): SessionState {
    const session = this.sessions.get(sessionId);
    if (!session) {
      throw new Error(`Session ${sessionId} not found`);
    }

    return session;
  }
}
