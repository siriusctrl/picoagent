import { test } from 'node:test';
import assert from 'node:assert';
import { Runtime } from '../../src/runtime/runtime.js';
import { MockProvider } from '../helpers/mock-provider.js';
import { join } from 'path';
import { ToolContext } from '../../src/core/types.js';

const testDir = join(process.cwd(), 'tests', 'temp-runtime');

test('Runtime processes user message', async () => {
    const provider = new MockProvider([
        { role: 'assistant', content: [{ type: 'text', text: 'Response 1' }] }
    ]);
    
    const context: ToolContext = {
        cwd: testDir,
        tasksRoot: testDir
    };

    const runtime = new Runtime(provider, [], [], context);

    let output = '';
    const result = await runtime.onUserMessage('Hello', (text) => output += text);

    assert.strictEqual(result.role, 'assistant');
    assert.strictEqual(result.content.length, 1);
    const textContent = result.content[0];
    if (textContent.type === 'text') {
        assert.strictEqual(textContent.text, 'Response 1');
    } else {
        assert.fail('Expected text content');
    }
    assert.strictEqual(output, 'Response 1');
});
