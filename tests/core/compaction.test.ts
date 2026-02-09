import { test } from 'node:test';
import assert from 'node:assert';
import { 
    estimateTokens, 
    extractFileOps, 
    compactMessages, 
    createCompactionHooks,
    CompactionConfig 
} from '../../src/hooks/compaction.js';
import { MockProvider } from '../helpers/mock-provider.js';
import { Message, AssistantMessage } from '../../src/core/types.js';

test('estimateTokens', () => {
    const messages: Message[] = [
        { role: 'user', content: 'hello' }, // 5
        { role: 'assistant', content: [{ type: 'text', text: 'world' }] } // 5
    ];
    // 10 chars / 1 char per token = 10
    assert.strictEqual(estimateTokens(messages, 1), 10);
    // 10 chars / 4 chars per token = 3
    assert.strictEqual(estimateTokens(messages, 4), 3);
});

test('extractFileOps', () => {
    const messages: Message[] = [
        { 
            role: 'assistant', 
            content: [
                { type: 'toolCall', id: '1', name: 'read_file', arguments: { path: '/a.txt' } },
                { type: 'toolCall', id: '2', name: 'write_file', arguments: { path: '/b.txt' } },
                { type: 'toolCall', id: '3', name: 'load', arguments: { path: '/c.md' } },
                { type: 'text', text: 'ignore me' }
            ] 
        }
    ];
    const ops = extractFileOps(messages);
    assert.deepStrictEqual(ops.read, ['/a.txt', '/c.md']);
    assert.deepStrictEqual(ops.modified, ['/b.txt']);
});

test('compactMessages reduces message count', async () => {
    const config: CompactionConfig = {
        contextWindow: 100,
        triggerRatio: 0.5,
        preserveRatio: 0.2, // Keep 20 chars
        charsPerToken: 1
    };
    
    // Create messages > 50 chars
    const messages: Message[] = [
        { role: 'user', content: '0123456789' }, // 10
        { role: 'user', content: '0123456789' }, // 10
        { role: 'user', content: '0123456789' }, // 10
        { role: 'user', content: '0123456789' }, // 10
        { role: 'user', content: '0123456789' }, // 10
        { role: 'user', content: 'keep me please' } // 14
    ]; // Total 64 chars > 50 trigger

    const summaryResponse: AssistantMessage = {
        role: 'assistant',
        content: [{ type: 'text', text: 'Summary of 50 chars' }]
    };
    const provider = new MockProvider([summaryResponse]);

    await compactMessages(messages, provider, config);

    // Should have cut the first 5 messages (50 chars)
    // Preserved 'keep me please' (14 chars) < 20 preserve limit
    // Result: [Summary, Keep me please]
    
    assert.strictEqual(messages.length, 2);
    assert.strictEqual(messages[0].role, 'user');
    const content = messages[0].content as string;
    assert.ok(typeof content === 'string');
    assert.ok(content.includes('## Previous Context'));
    assert.ok(content.includes('Summary of 50 chars'));
    assert.strictEqual(messages[1].content, 'keep me please');
});

test('compactMessages preserves recent messages', async () => {
    const config: CompactionConfig = {
        contextWindow: 100,
        triggerRatio: 0.5,
        preserveRatio: 0.4, // Keep 40 chars
        charsPerToken: 1
    };

    const messages: Message[] = [
        { role: 'user', content: 'old message 1' }, 
        { role: 'user', content: 'old message 2' },
        { role: 'user', content: 'recent 1 (20 chars)' }, // 19
        { role: 'user', content: 'recent 2 (20 chars)' }  // 19
    ]; 
    // Total length > 50?
    // "old message 1" = 13
    // "old message 2" = 13
    // "recent 1 (20 chars)" = 19
    // "recent 2 (20 chars)" = 19
    // Total = 64 > 50 trigger

    // Preserve 40 chars. 
    // recent 2 (19) + recent 1 (19) = 38 < 40.
    // old 2 (13) + 38 = 51 > 40.
    // So cut index should be at old 2. 
    // Messages preserved: recent 1, recent 2.

    const provider = new MockProvider([{ role: 'assistant', content: [{ type: 'text', text: 'Sum' }] }]);
    await compactMessages(messages, provider, config);

    const content1 = messages[1].content as string;
    const content2 = messages[2].content as string;
    assert.strictEqual(messages.length, 3); // Summary + 2 recent
    assert.strictEqual(content1, 'recent 1 (20 chars)');
    assert.strictEqual(content2, 'recent 2 (20 chars)');
});

test('createCompactionHooks integration', async () => {
     const config: CompactionConfig = {
        contextWindow: 100,
        triggerRatio: 0.5,
        preserveRatio: 0.2,
        charsPerToken: 1
    };
    const messages: Message[] = [
        { role: 'user', content: 'A'.repeat(60) } 
    ];
    
    const provider = new MockProvider([{ role: 'assistant', content: [{ type: 'text', text: 'Sum' }] }]);
    const hooks = createCompactionHooks(provider, config);
    
    if (hooks.onTurnEnd) {
        await hooks.onTurnEnd(messages);
    }

    const content = messages[0].content as string;
    assert.strictEqual(messages.length, 1);
    assert.ok(content.includes('## Previous Context'));
});

test('compactMessages updates existing summary', async () => {
    const config: CompactionConfig = {
        contextWindow: 100,
        triggerRatio: 0.2, // Low trigger
        preserveRatio: 0.1, 
        charsPerToken: 1
    };

    const messages: Message[] = [
        { role: 'user', content: '## Previous Context\nOld Summary' },
        { role: 'user', content: 'New stuff to summarize' },
        { role: 'user', content: 'Keep' }
    ];

    const provider = new MockProvider([{ role: 'assistant', content: [{ type: 'text', text: 'Updated Summary' }] }]);
    
    // We want to verify that the provider received the correct prompt containing "Old Summary"
    // MockProvider stores messages in `messages` property after complete.
    
    await compactMessages(messages, provider, config);
    
    // cast to any to access messages on MockProvider or define interface
    const lastPrompt = (provider as any).messages[0].content as string;
    const msgContent = messages[0].content as string;
    assert.ok(lastPrompt.includes('Old Summary'));
    assert.ok(lastPrompt.includes('New stuff to summarize'));
    assert.ok(msgContent.includes('Updated Summary'));
});
