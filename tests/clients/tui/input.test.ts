import { test } from 'node:test';
import assert from 'node:assert/strict';
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
} from '../../../src/clients/tui/input.js';

test('insertText inserts at the cursor and advances it', () => {
  assert.deepEqual(insertText({ value: 'heo', cursor: 2 }, 'l'), { value: 'helo', cursor: 3 });
});

test('cursor movement stays within bounds', () => {
  assert.deepEqual(moveCursorLeft({ value: 'abc', cursor: 0 }), { value: 'abc', cursor: 0 });
  assert.deepEqual(moveCursorRight({ value: 'abc', cursor: 3 }), { value: 'abc', cursor: 3 });
  assert.deepEqual(moveCursorHome({ value: 'abc', cursor: 2 }), { value: 'abc', cursor: 0 });
  assert.deepEqual(moveCursorEnd({ value: 'abc', cursor: 1 }), { value: 'abc', cursor: 3 });
});

test('delete operations remove characters around the cursor', () => {
  assert.deepEqual(deleteBackward({ value: 'abcd', cursor: 2 }), { value: 'acd', cursor: 1 });
  assert.deepEqual(deleteForward({ value: 'abcd', cursor: 2 }), { value: 'abd', cursor: 2 });
});

test('clear operations trim text to the cursor', () => {
  assert.deepEqual(clearToStart({ value: 'abcd', cursor: 2 }), { value: 'cd', cursor: 0 });
  assert.deepEqual(clearToEnd({ value: 'abcd', cursor: 2 }), { value: 'ab', cursor: 2 });
});

test('renderPrompt marks the cursor position for styled rendering', () => {
  assert.equal(renderPrompt('abc', 1, true), 'a\u0000b\u0001c');
  assert.equal(renderPrompt('abc', 3, true), 'abc\u0000 \u0001');
  assert.equal(renderPrompt('abc', 2, false), 'abc');
});

test('parseTerminalInput maps keyboard editing and viewport keys to terminal actions', () => {
  assert.deepEqual(parseTerminalInput('ab\x1b[A\x1b[B\x1b[C\x1b[D\x01\x05\x15\x0b\x1b[5~\x1b[6~\x1b[H\x1b[F\t\r').actions, [
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
    { type: 'toggle_mode' },
    { type: 'submit' },
  ]);
});

test('parseTerminalInput recognizes bracketed paste and SGR mouse wheel events', () => {
  assert.deepEqual(parseTerminalInput('\x1b[200~hi\nthere\x1b[201~\x1b[<64;80;12M\x1b[<65;80;13M').actions, [
    { type: 'insert_text', text: 'hi there' },
    { type: 'scroll_up', amount: 3 },
    { type: 'scroll_down', amount: 3 },
  ]);
});

test('parseTerminalInput recognizes legacy mouse wheel events without leaking bytes into input', () => {
  assert.deepEqual(parseTerminalInput('\x1b[M`!!\x1b[Ma!!').actions, [
    { type: 'scroll_up', amount: 3 },
    { type: 'scroll_down', amount: 3 },
  ]);
});

test('parseTerminalInput keeps incomplete escape sequences buffered', () => {
  assert.deepEqual(parseTerminalInput('\x1b[<64;80').rest, '\x1b[<64;80');
  assert.deepEqual(parseTerminalInput('\x1b[200~partial').rest, '\x1b[200~partial');
  assert.deepEqual(parseTerminalInput('\x1bO').rest, '\x1bO');
});

test('parseTerminalInput handles SS3 home/end sequences when they arrive after buffering', () => {
  assert.deepEqual(parseTerminalInput('\x1bOH\x1bOF').actions, [
    { type: 'history_home' },
    { type: 'history_end' },
  ]);
});
