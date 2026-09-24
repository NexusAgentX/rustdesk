# MCP protocol interoperability

`interop.mjs` uses the official TypeScript clients to exercise both supported
protocol revisions against the Rust module's test fixture. The fixture is only
compiled in tests and is never registered by the application.

Install Node.js 24 and the pinned clients outside the source tree:

```sh
mkdir -p /tmp/rustdesk-mcp-clients
npm install --prefix /tmp/rustdesk-mcp-clients --save-exact \
  @modelcontextprotocol/sdk@1.30.0 @modelcontextprotocol/client@2.0.0
```

With the application's native build prerequisites available, run from the repo:

```sh
MCP_CLIENT_DIRECTORY=/tmp/rustdesk-mcp-clients \
MCP_INTEROP_SCRIPT="$PWD/tests/mcp/interop.mjs" \
cargo test --features mcp official_typescript_clients -- --ignored --nocapture
```

The fixture binds an ephemeral loopback port and generates an ephemeral bearer
credential, passed to the child process through its environment. The test checks
client package versions, legacy initialization versus current discovery, tool
listing and calling, structured results, decoded PNG data, and the distinction
between tool execution failures and JSON-RPC errors. Current list caching fields
are checked on the HTTP response because the SDK's aggregation API removes them.
The ordinary module tests cover HTTP boundaries, authentication, cancellation,
timeouts and shutdown. These tests do not validate the product's business tools,
remote-control permissions or platform UI lifecycle.
