import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EchoProvider } from '../../src/providers/echo.js';
import { Message } from '../../src/core/types.js';

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

  assert.deepEqual(response, {
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

  assert.equal(deltas, 'received: stream me');
  assert.deepEqual(done?.message, {
    role: 'assistant',
    content: [{ type: 'text', text: 'received: stream me' }],
  });
});
