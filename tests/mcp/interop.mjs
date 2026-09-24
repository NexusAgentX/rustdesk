import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { inflateSync } from 'node:zlib';
import { join } from 'node:path';

const root = process.env.MCP_CLIENT_DIRECTORY;
assert(root, 'MCP_CLIENT_DIRECTORY must point to an isolated npm installation');
const require = createRequire(join(root, 'package.json'));
for (const [name, version] of [['sdk', '1.30.0'], ['client', '2.0.0']]) {
    const manifest = JSON.parse(readFileSync(join(root, 'node_modules', '@modelcontextprotocol', name, 'package.json')));
    assert.equal(manifest.version, version);
}

const legacy = require('@modelcontextprotocol/sdk/client/index.js');
const legacyHttp = require('@modelcontextprotocol/sdk/client/streamableHttp.js');
const current = require('@modelcontextprotocol/client');
const url = new URL(process.env.MCP_TEST_URL);
const authorization = `Bearer ${process.env.MCP_TEST_TOKEN}`;

function verifyPng(content) {
    assert.equal(content.mimeType, 'image/png');
    const bytes = Buffer.from(content.data, 'base64');
    assert.equal(bytes.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
    assert.equal(bytes.readUInt32BE(16), 1);
    assert.equal(bytes.readUInt32BE(20), 1);
    const compressed = [];
    for (let offset = 8; offset < bytes.length;) {
        const size = bytes.readUInt32BE(offset);
        const type = bytes.subarray(offset + 4, offset + 8).toString();
        if (type === 'IDAT') compressed.push(bytes.subarray(offset + 8, offset + 8 + size));
        offset += size + 12;
    }
    assert.equal(inflateSync(Buffer.concat(compressed)).length, 5);
}

for (const modern of [false, true]) {
    const messages = [];
    const replies = new Map();
    const observedFetch = async (input, init) => {
        if (init?.body) messages.push(JSON.parse(init.body));
        const response = await fetch(input, init);
        if (init?.body && response.headers.get('content-type')?.includes('application/json')) {
            replies.set(JSON.parse(init.body).method, await response.clone().json());
        }
        return response;
    };
    const Client = modern ? current.Client : legacy.Client;
    const Transport = modern ? current.StreamableHTTPClientTransport : legacyHttp.StreamableHTTPClientTransport;
    const options = modern ? { capabilities: {}, versionNegotiation: { mode: { pin: '2026-07-28' } } } : { capabilities: {} };
    const client = new Client({ name: 'rustdesk-protocol-test', version: '1' }, options);
    const transport = new Transport(url, { requestInit: { headers: { Authorization: authorization } }, fetch: observedFetch });
    try {
        await client.connect(transport);
        const list = await client.listTools();
        assert.deepEqual(list.tools.map(tool => tool.name), ['fixture']);
        const result = await client.callTool({ name: 'fixture', arguments: {} });
        assert.equal(result.isError, false);
        assert.equal(result.structuredContent.ok, true);
        verifyPng(result.content.find(content => content.type === 'image'));
        const failure = await client.callTool({ name: 'fixture', arguments: { mode: 'error' } });
        assert.equal(failure.isError, true);
        await assert.rejects(client.callTool({ name: 'fixture', arguments: { mode: 'invalid' } }));
        if (modern) {
            assert(messages.some(message => message.method === 'server/discover'));
            assert(!messages.some(message => message.method === 'initialize'));
            assert.equal(replies.get('tools/list').result.resultType, 'complete');
            assert.equal(replies.get('tools/list').result.cacheScope, 'private');
        } else {
            assert(messages.some(message => message.method === 'initialize'));
            assert(messages.some(message => message.method === 'notifications/initialized'));
        }
        console.log(`${modern ? '2026-07-28 / client 2.0.0' : '2025-11-25 / sdk 1.30.0'}: discovery, tools, structured result, PNG and errors passed`);
    } finally {
        await client.close();
    }
}
