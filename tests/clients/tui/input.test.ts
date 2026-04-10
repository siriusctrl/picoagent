import { test, expect } from 'bun:test';
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
} from '../../../src/clients/tui/input.ts';

test('insertText inserts at the cursor and advances it', () => {
  expect(insertText({ value: 'heo', cursor: 2 }, 'l')).toEqual({ value: 'helo', cursor: 3 });
});

test('cursor movement stays within bounds', () => {
  expect(moveCursorLeft({ value: 'abc', cursor: 0 })).toEqual({ value: 'abc', cursor: 0 });
  expect(moveCursorRight({ value: 'abc', cursor: 3 })).toEqual({ value: 'abc', cursor: 3 });
  expect(moveCursorHome({ value: 'abc', cursor: 2 })).toEqual({ value: 'abc', cursor: 0 });
  expect(moveCursorEnd({ value: 'abc', cursor: 1 })).toEqual({ value: 'abc', cursor: 3 });
});

test('delete operations remove characters around the cursor', () => {
  expect(deleteBackward({ value: 'abcd', cursor: 2 })).toEqual({ value: 'acd', cursor: 1 });
  expect(deleteForward({ value: 'abcd', cursor: 2 })).toEqual({ value: 'abd', cursor: 2 });
});

test('clear operations trim text to the cursor', () => {
  expect(clearToStart({ value: 'abcd', cursor: 2 })).toEqual({ value: 'cd', cursor: 0 });
  expect(clearToEnd({ value: 'abcd', cursor: 2 })).toEqual({ value: 'ab', cursor: 2 });
});

test('renderPrompt marks the cursor position for styled rendering', () => {
  expect(renderPrompt('abc', 1, true)).toBe('a\u0000b\u0001c');
  expect(renderPrompt('abc', 3, true)).toBe('abc\u0000 \u0001');
  expect(renderPrompt('abc', 2, false)).toBe('abc');
});

test('parseTerminalInput maps keyboard editing and viewport keys to terminal actions', () => {
  expect(
    parseTerminalInput('ab\x1b[A\x1b[B\x1b[C\x1b[D\x01\x05\x15\x0b\x1b[5~\x1b[6~\x1b[H\x1b[F\t\r').actions,
  ).toEqual([
    { type: 'insert_text', text: 'ab' },
    { type: 'prompt_history_up' },
    { type: 'prompt_history_down' },
    { type: 'cursor_right' },
    { type: 'cursor_left' },
    { type: 'cursor_home' },
    { type: 'cursor_end' },
    { type: 'clear_to_start' },
    { type: 'clear_to_end' },
    { type: 'history_page_up' },
    { type: 'history_page_down' },
    { type: 'history_home' },
    { type: 'history_end' },
    { type: 'toggle_agent' },
    { type: 'submit' },
  ]);
});

test('parseTerminalInput recognizes bracketed paste and SGR mouse wheel events', () => {
  expect(
    parseTerminalInput('\x1b[200~hi\nthere\x1b[201~\x1b[<64;80;12M\x1b[<65;80;13M').actions,
  ).toEqual([
    { type: 'insert_text', text: 'hi there' },
    { type: 'scroll_up', amount: 3 },
    { type: 'scroll_down', amount: 3 },
  ]);
});

test('parseTerminalInput recognizes legacy mouse wheel events without leaking bytes into input', () => {
  expect(parseTerminalInput('\x1b[M`!!\x1b[Ma!!').actions).toEqual([
    { type: 'scroll_up', amount: 3 },
    { type: 'scroll_down', amount: 3 },
  ]);
});

test('parseTerminalInput keeps incomplete escape sequences buffered', () => {
  expect(parseTerminalInput('\x1b[<64;80').rest).toBe('\x1b[<64;80');
  expect(parseTerminalInput('\x1b[200~partial').rest).toBe('\x1b[200~partial');
  expect(parseTerminalInput('\x1bO').rest).toBe('\x1bO');
});

test('parseTerminalInput handles SS3 home/end sequences when they arrive after buffering', () => {
  expect(parseTerminalInput('\x1bOH\x1bOF').actions).toEqual([
    { type: 'history_home' },
    { type: 'history_end' },
  ]);
});
