import { expect, test } from 'bun:test';
import { EchoProvider } from '../../src/providers/echo.ts';
import { Message } from '../../src/core/types.ts';

test('echo provider completes with the last user message', async () => {
  const provider = new EchoProvider({
    apiKey: '',
    model: 'echo',
  });
  const messages: Message[] = [
    { role: 'user', content: 'first' },
    { role: 'assistant', content: [{ type: 'text', text: 'ignored' }] },
    { role: 'user', content: 'hello world' },
  ];

  const response = await provider.complete(messages, []);

  expect(response).toEqual({
    role: 'assistant',
    content: [{ type: 'text', text: 'received: hello world' }],
  });
});

test('echo provider streams text deltas before the final message', async () => {
  const provider = new EchoProvider({
    apiKey: '',
    model: 'echo',
  });
  const events = [];

  for await (const event of provider.stream([{ role: 'user', content: 'stream me' }], [])) {
    events.push(event);
  }

  const deltas = events.filter((event) => event.type === 'text_delta').map((event) => event.text).join('');
  const done = events.find((event) => event.type === 'done');

  expect(deltas).toBe('received: stream me');
  expect(done?.message).toEqual({
    role: 'assistant',
    content: [{ type: 'text', text: 'received: stream me' }],
  });
});
