import { test } from 'node:test';
import assert from 'node:assert';
import { WorkerControl, createWorkerControlHooks, AbortError } from '../../src/core/worker-control.js';
import { Message, ToolCall, ToolResultMessage } from '../../src/core/types.js';

test('WorkerControl abort', async () => {
    const control = new WorkerControl();
    assert.strictEqual(control.aborted, false);
    control.abort();
    assert.strictEqual(control.aborted, true);
});

test('WorkerControl steer', async () => {
    const control = new WorkerControl();
    control.steer('go left');
    control.steer('go right');
    assert.strictEqual(control.consumeSteer(), 'go left');
    assert.strictEqual(control.consumeSteer(), 'go right');
    assert.strictEqual(control.consumeSteer(), undefined);
});

test('WorkerControl hooks check abort on tool end', async () => {
    const control = new WorkerControl();
    const hooks = createWorkerControlHooks(control, 't_001');
    const call: ToolCall = { type: 'toolCall', id: '1', name: 'test', arguments: {} };
    const result: ToolResultMessage = { role: 'toolResult', toolCallId: '1', content: 'ok', isError: false };

    // Not aborted
    await hooks.onToolEnd?.(call, result, 0);

    // Aborted
    control.abort();
    try {
        await hooks.onToolEnd?.(call, result, 0);
        assert.fail('Should have thrown AbortError');
    } catch (e) {
        assert.ok(e instanceof AbortError);
    }
});

test('WorkerControl hooks inject steer messages on turn end', async () => {
    const control = new WorkerControl();
    const hooks = createWorkerControlHooks(control, 't_001');
    const messages: Message[] = [];

    control.steer('new direction');
    await hooks.onTurnEnd?.(messages);

    assert.strictEqual(messages.length, 1);
    assert.strictEqual(messages[0].role, 'user');
    assert.strictEqual(messages[0].content, '[Steer] new direction');
});
