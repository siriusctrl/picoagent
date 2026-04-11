export interface PromptState {
  value: string;
  cursor: number;
}

export type TerminalAction =
  | { type: 'submit' }
  | { type: 'interrupt' }
  | { type: 'prompt_history_up' }
  | { type: 'prompt_history_down' }
  | { type: 'history_page_up' }
  | { type: 'history_page_down' }
  | { type: 'history_home' }
  | { type: 'history_end' }
  | { type: 'cursor_left' }
  | { type: 'cursor_right' }
  | { type: 'cursor_home' }
  | { type: 'cursor_end' }
  | { type: 'delete_backward' }
  | { type: 'delete_forward' }
  | { type: 'clear_to_start' }
  | { type: 'clear_to_end' }
  | { type: 'scroll_up'; amount: number }
  | { type: 'scroll_down'; amount: number }
  | { type: 'insert_text'; text: string };

export interface ParsedTerminalInput {
  actions: TerminalAction[];
  rest: string;
}

function decodeLegacyMouseButton(byte: number): number {
  return byte - 32;
}

function isControlCharacter(character: string): boolean {
  return character < ' ' || character === '\x7f';
}

function parseCsiTilde(sequence: string): TerminalAction | null {
  switch (sequence) {
    case '1':
    case '7':
      return { type: 'history_home' };
    case '3':
      return { type: 'delete_forward' };
    case '4':
    case '8':
      return { type: 'history_end' };
    case '5':
      return { type: 'history_page_up' };
    case '6':
      return { type: 'history_page_down' };
    default:
      return null;
  }
}

export function parseTerminalInput(data: string): ParsedTerminalInput {
  const actions: TerminalAction[] = [];
  let index = 0;

  while (index < data.length) {
    const current = data[index];

    if (current === '\x03') {
      actions.push({ type: 'interrupt' });
      index += 1;
      continue;
    }

    if (current === '\r') {
      actions.push({ type: 'submit' });
      index += data[index + 1] === '\n' ? 2 : 1;
      continue;
    }

    if (current === '\n') {
      actions.push({ type: 'submit' });
      index += 1;
      continue;
    }

    if (current === '\x7f') {
      actions.push({ type: 'delete_backward' });
      index += 1;
      continue;
    }

    if (current === '\x01') {
      actions.push({ type: 'cursor_home' });
      index += 1;
      continue;
    }

    if (current === '\x05') {
      actions.push({ type: 'cursor_end' });
      index += 1;
      continue;
    }

    if (current === '\x15') {
      actions.push({ type: 'clear_to_start' });
      index += 1;
      continue;
    }

    if (current === '\x0b') {
      actions.push({ type: 'clear_to_end' });
      index += 1;
      continue;
    }

    if (current === '\x1b') {
      const remaining = data.slice(index);

      if (remaining.length === 1) {
        return { actions, rest: remaining };
      }

      if (remaining === '\x1bO') {
        return { actions, rest: remaining };
      }

      if (remaining.startsWith('\x1b[200~')) {
        const end = remaining.indexOf('\x1b[201~');
        if (end === -1) {
          return { actions, rest: remaining };
        }

        const pasted = remaining.slice('\x1b[200~'.length, end).replace(/\r?\n/g, ' ');
        if (pasted.length > 0) {
          actions.push({ type: 'insert_text', text: pasted });
        }
        index += end + '\x1b[201~'.length;
        continue;
      }

      const mouseMatch = remaining.match(/^\x1b\[<(\d+);(\d+);(\d+)([mM])/);
      if (mouseMatch) {
        const button = Number(mouseMatch[1]);
        if (button === 64) {
          actions.push({ type: 'scroll_up', amount: 3 });
        } else if (button === 65) {
          actions.push({ type: 'scroll_down', amount: 3 });
        }

        index += mouseMatch[0].length;
        continue;
      }

      if (remaining.startsWith('\x1b[M')) {
        if (remaining.length < 6) {
          return { actions, rest: remaining };
        }

        const button = decodeLegacyMouseButton(remaining.charCodeAt(3));
        if (button === 64) {
          actions.push({ type: 'scroll_up', amount: 3 });
        } else if (button === 65) {
          actions.push({ type: 'scroll_down', amount: 3 });
        }

        index += 6;
        continue;
      }

      if (/^\x1b\[<[\d;]*$/.test(remaining)) {
        return { actions, rest: remaining };
      }

      const csiTilde = remaining.match(/^\x1b\[(\d+)~/);
      if (csiTilde) {
        const action = parseCsiTilde(csiTilde[1]);
        if (action) {
          actions.push(action);
        }
        index += csiTilde[0].length;
        continue;
      }

      if (/^\x1b\[[\d;]*$/.test(remaining)) {
        return { actions, rest: remaining };
      }

      if (remaining.startsWith('\x1b[A')) {
        actions.push({ type: 'prompt_history_up' });
        index += 3;
        continue;
      }

      if (remaining.startsWith('\x1b[B')) {
        actions.push({ type: 'prompt_history_down' });
        index += 3;
        continue;
      }

      if (remaining.startsWith('\x1b[C')) {
        actions.push({ type: 'cursor_right' });
        index += 3;
        continue;
      }

      if (remaining.startsWith('\x1b[D')) {
        actions.push({ type: 'cursor_left' });
        index += 3;
        continue;
      }

      if (remaining.startsWith('\x1b[H') || remaining.startsWith('\x1bOH')) {
        actions.push({ type: 'history_home' });
        index += 3;
        continue;
      }

      if (remaining.startsWith('\x1b[F') || remaining.startsWith('\x1bOF')) {
        actions.push({ type: 'history_end' });
        index += 3;
        continue;
      }

      index += 1;
      continue;
    }

    if (isControlCharacter(current)) {
      index += 1;
      continue;
    }

    let end = index + 1;
    while (end < data.length && data[end] !== '\x1b' && !isControlCharacter(data[end])) {
      end += 1;
    }

    actions.push({ type: 'insert_text', text: data.slice(index, end) });
    index = end;
  }

  return { actions, rest: '' };
}

export function clampCursor(value: string, cursor: number): number {
  return Math.max(0, Math.min(cursor, value.length));
}

export function insertText(state: PromptState, text: string): PromptState {
  const cursor = clampCursor(state.value, state.cursor);
  return {
    value: `${state.value.slice(0, cursor)}${text}${state.value.slice(cursor)}`,
    cursor: cursor + text.length,
  };
}

export function moveCursorLeft(state: PromptState): PromptState {
  return { ...state, cursor: Math.max(0, state.cursor - 1) };
}

export function moveCursorRight(state: PromptState): PromptState {
  return { ...state, cursor: Math.min(state.value.length, state.cursor + 1) };
}

export function moveCursorHome(state: PromptState): PromptState {
  return { ...state, cursor: 0 };
}

export function moveCursorEnd(state: PromptState): PromptState {
  return { ...state, cursor: state.value.length };
}

export function deleteBackward(state: PromptState): PromptState {
  const cursor = clampCursor(state.value, state.cursor);
  if (cursor === 0) {
    return state;
  }

  return {
    value: `${state.value.slice(0, cursor - 1)}${state.value.slice(cursor)}`,
    cursor: cursor - 1,
  };
}

export function deleteForward(state: PromptState): PromptState {
  const cursor = clampCursor(state.value, state.cursor);
  if (cursor >= state.value.length) {
    return state;
  }

  return {
    value: `${state.value.slice(0, cursor)}${state.value.slice(cursor + 1)}`,
    cursor,
  };
}

export function clearToStart(state: PromptState): PromptState {
  const cursor = clampCursor(state.value, state.cursor);
  return {
    value: state.value.slice(cursor),
    cursor: 0,
  };
}

export function clearToEnd(state: PromptState): PromptState {
  const cursor = clampCursor(state.value, state.cursor);
  return {
    value: state.value.slice(0, cursor),
    cursor,
  };
}

export function renderPrompt(value: string, cursor: number, focused: boolean): string {
  const clamped = clampCursor(value, cursor);
  const before = value.slice(0, clamped);
  const current = value[clamped] ?? ' ';
  const after = value.slice(clamped + (clamped < value.length ? 1 : 0));

  if (!focused) {
    return value;
  }

  return `${before}\u0000${current}\u0001${after}`;
}
