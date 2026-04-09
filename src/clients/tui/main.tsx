import React, { useEffect, useEffectEvent, useRef, useState } from 'react';
import { randomUUID } from 'node:crypto';
import { Box, render, Text, useApp, useStdin, useWindowSize } from 'ink';
import { TuiController, UiEvent } from './controller.js';
import { SessionModeId } from '../../core/types.js';
import { clampScrollOffset, getHistoryWindow, preserveScrollOffsetOnAppend } from './history.js';
import { estimateEntryHeight } from './layout.js';
import {
  clearToEnd,
  clearToStart,
  deleteBackward,
  deleteForward,
  insertText,
  moveCursorEnd,
  moveCursorHome,
  moveCursorLeft,
  moveCursorRight,
  parseTerminalInput,
  renderPrompt,
  TerminalAction,
} from './input.js';

type Entry =
  | { id: string; type: 'system' | 'error'; text: string }
  | { id: string; type: 'user' | 'assistant'; text: string }
  | { id: string; type: 'tool'; title: string; status: string; output?: string };

const LABEL_WIDTH = 8;
const MIN_HISTORY_ROWS = 6;
const SHOW_INPUT_DEBUG = process.env.PICO_TUI_DEBUG_INPUT === '1';

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

function entryTone(entry: Entry): { label: string; color: string } {
  switch (entry.type) {
    case 'user':
      return { label: 'you', color: 'blueBright' };
    case 'assistant':
      return { label: 'agent', color: 'white' };
    case 'tool':
      return { label: 'tool', color: 'magentaBright' };
    case 'error':
      return { label: 'error', color: 'redBright' };
    case 'system':
      return { label: 'system', color: 'gray' };
  }
}

function HistoryEntry({ entry }: { entry: Entry }) {
  const tone = entryTone(entry);

  if (entry.type === 'tool') {
    return (
      <Box key={entry.id} marginBottom={1}>
        <Box width={LABEL_WIDTH}>
          <Text color={tone.color}>{tone.label}</Text>
        </Box>
        <Box flexDirection="column" flexGrow={1}>
          <Text color={tone.color}>
            {entry.title} [{entry.status}]
          </Text>
          {entry.output ? <Text color="gray">{entry.output}</Text> : null}
        </Box>
      </Box>
    );
  }

  return (
    <Box key={entry.id} marginBottom={1}>
      <Box width={LABEL_WIDTH}>
        <Text color={tone.color}>{tone.label}</Text>
      </Box>
      <Box flexGrow={1}>
        <Text color={tone.color}>{entry.text}</Text>
      </Box>
    </Box>
  );
}

function PromptLine({
  value,
  cursor,
  focused,
  color,
}: {
  value: string;
  cursor: number;
  focused: boolean;
  color: string;
}) {
  const rendered = renderPrompt(value, cursor, focused);
  const markerStart = rendered.indexOf('\u0000');
  const markerEnd = rendered.indexOf('\u0001');

  if (markerStart === -1 || markerEnd === -1) {
    return <Text color={color}>{rendered || ' '}</Text>;
  }

  return (
    <Text color={color}>
      {rendered.slice(0, markerStart)}
      <Text inverse>{rendered.slice(markerStart + 1, markerEnd)}</Text>
      {rendered.slice(markerEnd + 1)}
    </Text>
  );
}

function App() {
  const { exit } = useApp();
  const { stdin, setRawMode, isRawModeSupported } = useStdin();
  const windowSize = useWindowSize();
  const [controller, setController] = useState<TuiController | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [input, setInput] = useState({ value: '', cursor: 0 });
  const [mode, setMode] = useState<SessionModeId>('ask');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('Starting ACP session...');
  const [debugInput, setDebugInput] = useState<string>('');
  const [promptHistory, setPromptHistory] = useState<string[]>([]);
  const [promptHistoryIndex, setPromptHistoryIndex] = useState(-1);
  const [scrollOffset, setScrollOffset] = useState(0);
  const previousEntryHeights = useRef<number[]>([]);
  const inputDraft = useRef('');
  const inputRef = useRef(input);
  const busyRef = useRef(busy);
  const promptHistoryRef = useRef(promptHistory);
  const promptHistoryIndexRef = useRef(promptHistoryIndex);
  const historyContentWidth = Math.max(windowSize.columns - LABEL_WIDTH - 4, 16);
  const historyHeights = entries.map((entry) => estimateEntryHeight(entry, historyContentWidth));
  const viewportSize = Math.max(windowSize.rows - (SHOW_INPUT_DEBUG ? 9 : 8), MIN_HISTORY_ROWS);

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

  useEffect(() => {
    if (promptHistoryIndex === -1) {
      inputDraft.current = input.value;
    }
  }, [input.value, promptHistoryIndex]);

  useEffect(() => {
    inputRef.current = input;
  }, [input]);

  useEffect(() => {
    busyRef.current = busy;
  }, [busy]);

  useEffect(() => {
    promptHistoryRef.current = promptHistory;
  }, [promptHistory]);

  useEffect(() => {
    promptHistoryIndexRef.current = promptHistoryIndex;
  }, [promptHistoryIndex]);

  useEffect(() => {
    setScrollOffset((current) =>
      preserveScrollOffsetOnAppend(previousEntryHeights.current, historyHeights, viewportSize, current),
    );
    previousEntryHeights.current = historyHeights;
  }, [historyHeights, viewportSize]);

  useEffect(() => {
    if (isRawModeSupported) {
      setRawMode(true);
      return () => {
        setRawMode(false);
      };
    }
  }, [isRawModeSupported, setRawMode]);

  useEffect(() => {
    if (!process.stdout.isTTY) {
      return;
    }

    process.stdout.write('\u001b[?1000h\u001b[?1002h\u001b[?1003h\u001b[?1006h');
    return () => {
      process.stdout.write('\u001b[?1006l\u001b[?1003l\u001b[?1002l\u001b[?1000l');
    };
  }, []);

  const updateInput = (next: { value: string; cursor: number } | ((current: { value: string; cursor: number }) => { value: string; cursor: number })) => {
    const resolved = typeof next === 'function' ? next(inputRef.current) : next;
    inputRef.current = resolved;
    setInput(resolved);
  };

  const replaceInput = (value: string) => {
    updateInput({ value, cursor: value.length });
  };

  const submit = useEffectEvent(async (text: string) => {
    if (!controller || busyRef.current || !text.trim()) {
      return;
    }

    setEntries((current) => [...current, { id: randomUUID(), type: 'user', text }]);
    promptHistoryRef.current = [...promptHistoryRef.current, text];
    setPromptHistory(promptHistoryRef.current);
    promptHistoryIndexRef.current = -1;
    setPromptHistoryIndex(-1);
    inputDraft.current = '';
    inputRef.current = { value: '', cursor: 0 };
    setInput(inputRef.current);
    busyRef.current = true;
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
      busyRef.current = false;
      setBusy(false);
    }
  });

  const handleTerminalAction = useEffectEvent((action: TerminalAction) => {
    if (action.type === 'interrupt') {
      exit();
      return;
    }

    if (action.type === 'prompt_history_up') {
      if (busyRef.current || promptHistoryRef.current.length === 0) {
        return;
      }

      const currentIndex = promptHistoryIndexRef.current;
      const nextIndex = currentIndex === -1 ? promptHistoryRef.current.length - 1 : Math.max(currentIndex - 1, 0);
      if (currentIndex === -1) {
        inputDraft.current = inputRef.current.value;
      }
      promptHistoryIndexRef.current = nextIndex;
      setPromptHistoryIndex(nextIndex);
      replaceInput(promptHistoryRef.current[nextIndex]);
      return;
    }

    if (action.type === 'prompt_history_down') {
      if (busyRef.current || promptHistoryIndexRef.current === -1) {
        return;
      }

      const nextIndex = promptHistoryIndexRef.current + 1;
      if (nextIndex >= promptHistoryRef.current.length) {
        replaceInput(inputDraft.current);
        promptHistoryIndexRef.current = -1;
        setPromptHistoryIndex(-1);
        return;
      }

      promptHistoryIndexRef.current = nextIndex;
      setPromptHistoryIndex(nextIndex);
      replaceInput(promptHistoryRef.current[nextIndex]);
      return;
    }

    if (action.type === 'history_page_up') {
      setScrollOffset((current) => clampScrollOffset(historyHeights, viewportSize, current + viewportSize));
      return;
    }

    if (action.type === 'history_page_down') {
      setScrollOffset((current) => clampScrollOffset(historyHeights, viewportSize, current - viewportSize));
      return;
    }

    if (action.type === 'history_home') {
      setScrollOffset(clampScrollOffset(historyHeights, viewportSize, Number.POSITIVE_INFINITY));
      return;
    }

    if (action.type === 'history_end') {
      setScrollOffset(0);
      return;
    }

    if (action.type === 'scroll_up') {
      setScrollOffset((current) => clampScrollOffset(historyHeights, viewportSize, current + action.amount));
      return;
    }

    if (action.type === 'scroll_down') {
      setScrollOffset((current) => clampScrollOffset(historyHeights, viewportSize, current - action.amount));
      return;
    }

    if (action.type === 'toggle_mode' && controller && !busyRef.current) {
      const targetMode = nextMode(mode);
      void controller.setMode(targetMode).catch((error: unknown) => {
        handleEvent({
          type: 'error',
          text: error instanceof Error ? error.message : String(error),
        });
      });
      return;
    }

    if (busyRef.current) {
      return;
    }

    if (action.type === 'submit') {
      void submit(inputRef.current.value);
      return;
    }

    if (action.type === 'cursor_left') {
      updateInput((current) => moveCursorLeft(current));
      return;
    }

    if (action.type === 'cursor_right') {
      updateInput((current) => moveCursorRight(current));
      return;
    }

    if (action.type === 'delete_backward') {
      updateInput((current) => deleteBackward(current));
      return;
    }

    if (action.type === 'delete_forward') {
      updateInput((current) => deleteForward(current));
      return;
    }

    if (action.type === 'cursor_home') {
      updateInput((current) => moveCursorHome(current));
      return;
    }

    if (action.type === 'cursor_end') {
      updateInput((current) => moveCursorEnd(current));
      return;
    }

    if (action.type === 'clear_to_start') {
      updateInput((current) => clearToStart(current));
      return;
    }

    if (action.type === 'clear_to_end') {
      updateInput((current) => clearToEnd(current));
      return;
    }

    if (action.type === 'insert_text' && action.text) {
      updateInput((current) => insertText(current, action.text.replace(/\r?\n/g, '')));
    }
  });

  useEffect(() => {
    let rest = '';

    const onData = (chunk: Buffer | string) => {
      const raw = `${rest}${chunk.toString()}`;
      if (SHOW_INPUT_DEBUG) {
        setDebugInput(JSON.stringify(raw));
      }

      const parsed = parseTerminalInput(raw);
      rest = parsed.rest;
      for (const action of parsed.actions) {
        handleTerminalAction(action);
      }
    };

    stdin.on('data', onData);
    return () => {
      stdin.off('data', onData);
    };
  }, [stdin]);

  const historyWindow = getHistoryWindow(historyHeights, viewportSize, scrollOffset);
  const visibleEntries = entries.slice(historyWindow.start, historyWindow.end);
  const olderCount = historyWindow.scrollOffset;
  const newerCount = historyWindow.maxScrollOffset - historyWindow.scrollOffset;
  const divider = '─'.repeat(Math.max(windowSize.columns - 2, 8));
  const scrollMeta = olderCount > 0 || newerCount > 0 ? `older ${olderCount}  newer ${newerCount}` : 'latest';

  return (
    <Box flexDirection="column" paddingX={1}>
      <Box justifyContent="space-between">
        <Text color="cyanBright">picoagent</Text>
        <Text color={mode === 'ask' ? 'yellowBright' : 'greenBright'}>{mode}</Text>
      </Box>
      <Box justifyContent="space-between">
        <Text color="gray">{status}</Text>
        <Text color="gray">{scrollMeta}</Text>
      </Box>
      {SHOW_INPUT_DEBUG ? <Text color="magentaBright">input {debugInput || '(none)'}</Text> : null}
      <Text color="gray">Enter send, wheel or PgUp/PgDn/Home/End scroll, Up/Down prompt history, Tab mode, Ctrl+C quit</Text>
      <Box marginTop={1}>
        <Text color="gray">{divider}</Text>
      </Box>
      <Box flexDirection="column" flexGrow={1} marginTop={1} marginBottom={1}>
        {visibleEntries.length > 0 ? (
          visibleEntries.map((entry) => <HistoryEntry key={entry.id} entry={entry} />)
        ) : (
          <Text color="gray">No conversation yet.</Text>
        )}
      </Box>
      <Box>
        <Text color="gray">{divider}</Text>
      </Box>
      <Box marginTop={1}>
        <Box width={LABEL_WIDTH}>
          <Text color={busy ? 'yellowBright' : 'cyanBright'}>{busy ? 'wait' : 'send'}</Text>
        </Box>
        <Box flexGrow={1}>
          <Text color={busy ? 'yellowBright' : 'cyanBright'}>› </Text>
          <PromptLine value={input.value} cursor={input.cursor} focused={!busy} color={busy ? 'yellowBright' : 'white'} />
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
