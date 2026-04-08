import React, { useEffect, useEffectEvent, useState } from 'react';
import { randomUUID } from 'node:crypto';
import { Box, render, Text, useApp, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { TuiController, UiEvent } from './controller.js';
import { SessionModeId } from '../core/types.js';

type Entry =
  | { id: string; type: 'system' | 'error'; text: string }
  | { id: string; type: 'user' | 'assistant'; text: string }
  | { id: string; type: 'tool'; title: string; status: string; output?: string };

function nextMode(mode: SessionModeId): SessionModeId {
  return mode === 'ask' ? 'exec' : 'ask';
}

function toolOutputText(rawOutput: unknown, fallback?: string): string | undefined {
  if (rawOutput && typeof rawOutput === 'object' && 'output' in rawOutput && typeof rawOutput.output === 'string') {
    return rawOutput.output;
  }

  return fallback;
}

function enableTerminalScreen(): (() => void) | undefined {
  if (!process.stdout.isTTY) {
    return undefined;
  }

  process.stdout.write('\u001b[?1049h\u001b[2J\u001b[H');
  return () => {
    process.stdout.write('\u001b[?1049l');
  };
}

const restoreTerminalScreen = enableTerminalScreen();

function App() {
  const { exit } = useApp();
  const [controller, setController] = useState<TuiController | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [input, setInput] = useState('');
  const [mode, setMode] = useState<SessionModeId>('ask');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('Starting ACP session...');

  const handleEvent = useEffectEvent((event: UiEvent) => {
    switch (event.type) {
      case 'status':
        setStatus(event.text);
        return;
      case 'mode':
        setMode(event.mode);
        return;
      case 'assistant_delta':
        setEntries((current) => {
          const last = current[current.length - 1];
          if (last?.type === 'assistant') {
            return [...current.slice(0, -1), { ...last, text: last.text + event.text }];
          }

          return [...current, { id: randomUUID(), type: 'assistant', text: event.text }];
        });
        return;
      case 'tool_call':
        setEntries((current) => [
          ...current,
          {
            id: event.toolCallId,
            type: 'tool',
            title: event.title,
            status: event.status ?? 'pending',
          },
        ]);
        return;
      case 'tool_call_update':
        setEntries((current) =>
          current.map((entry) =>
            entry.type === 'tool' && entry.id === event.toolCallId
              ? {
                  ...entry,
                  title: event.title ?? entry.title,
                  status: event.status ?? entry.status,
                  output: toolOutputText(event.rawOutput, event.text),
                }
              : entry,
          ),
        );
        return;
      case 'error':
        setEntries((current) => [...current, { id: randomUUID(), type: 'error', text: event.text }]);
        setStatus(event.text);
        return;
    }
  });

  useEffect(() => {
    const nextController = new TuiController({
      cwd: process.cwd(),
      onEvent: handleEvent,
    });
    setController(nextController);

    void nextController.start().catch((error: unknown) => {
      handleEvent({
        type: 'error',
        text: error instanceof Error ? error.message : String(error),
      });
    });

    return () => {
      void nextController.stop();
    };
  }, []);

  useInput((value, key) => {
    if (key.ctrl && value === 'c') {
      exit();
      return;
    }

    if (key.tab && controller && !busy) {
      const targetMode = nextMode(mode);
      void controller.setMode(targetMode).catch((error: unknown) => {
        handleEvent({
          type: 'error',
          text: error instanceof Error ? error.message : String(error),
        });
      });
    }
  });

  const submit = async (text: string) => {
    if (!controller || busy || !text.trim()) {
      return;
    }

    setEntries((current) => [...current, { id: randomUUID(), type: 'user', text }]);
    setInput('');
    setBusy(true);
    setStatus(`Running in ${mode} mode...`);

    try {
      await controller.sendPrompt(text);
      setStatus(`Ready in ${mode} mode`);
    } catch (error: unknown) {
      handleEvent({
        type: 'error',
        text: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
    }
  };

  const visibleEntries = entries.slice(-24);

  return (
    <Box flexDirection="column" paddingX={1}>
      <Box justifyContent="space-between" marginBottom={1}>
        <Text color="cyanBright">picoagent</Text>
        <Text color={mode === 'ask' ? 'yellowBright' : 'greenBright'}>{mode}</Text>
      </Box>
      <Text color="gray">{status}</Text>
      <Text color="gray">Enter send, Tab switch mode, Ctrl+C quit</Text>
      <Box flexDirection="column" marginTop={1} marginBottom={1}>
        {visibleEntries.map((entry) => {
          if (entry.type === 'tool') {
            return (
              <Box key={entry.id} flexDirection="column" marginBottom={1}>
                <Text color="magentaBright">tool {entry.title} [{entry.status}]</Text>
                {entry.output ? <Text color="gray">{entry.output}</Text> : null}
              </Box>
            );
          }

          const color =
            entry.type === 'user'
              ? 'blueBright'
              : entry.type === 'assistant'
                ? 'white'
                : entry.type === 'error'
                  ? 'redBright'
                  : 'gray';

          const label =
            entry.type === 'user'
              ? 'you'
              : entry.type === 'assistant'
                ? 'agent'
                : entry.type === 'error'
                  ? 'error'
                  : 'system';

          return (
            <Text key={entry.id} color={color}>
              {label}: {entry.text}
            </Text>
          );
        })}
      </Box>
      <Box borderStyle="round" borderColor={busy ? 'yellow' : 'cyan'}>
        <Box paddingX={1} width={8}>
          <Text>{busy ? 'wait' : 'input'}</Text>
        </Box>
        <Box flexGrow={1}>
          <TextInput value={input} onChange={setInput} onSubmit={submit} focus={!busy} />
        </Box>
      </Box>
    </Box>
  );
}

const instance = render(<App />);

const cleanup = () => {
  restoreTerminalScreen?.();
};

instance.waitUntilExit().finally(cleanup);
