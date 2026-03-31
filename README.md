# http-client

HTTP client ACT component — make outbound HTTP/HTTPS requests from a sandboxed WASM component.

## Usage

```bash
just init   # first time: fetch WIT deps
just build  # build wasm component
just test   # run e2e tests
```

```bash
# Serve over HTTP
act run http_client.wasm

# Serve over MCP (e.g. for Claude Desktop)
act run --mcp http_client.wasm

# Direct call
act call http_client.wasm fetch --args '{"url": "https://httpbin.org/get"}'
```

## Tool: fetch

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `url` | string | required | URL to fetch |
| `method` | string | `GET` | HTTP method |
| `headers` | object | `{}` | Request headers |
| `body` | string | — | Text body (UTF-8) |
| `body_json` | any | — | JSON body (auto-serialized, auto-sets Content-Type) |
| `body_raw` | bytes | — | Raw binary body |
| `timeout_ms` | number | — | Request timeout in milliseconds |
| `follow_redirects` | boolean | `true` | Follow redirects (up to 10 hops) |

Only one body type per request: `body`, `body_json`, or `body_raw`.

### Examples

```bash
# Simple GET
act call http_client.wasm fetch --args '{"url": "https://httpbin.org/get"}'

# POST JSON
act call http_client.wasm fetch --args '{
  "url": "https://api.example.com/data",
  "method": "POST",
  "body_json": {"key": "value"},
  "headers": {"Authorization": "Bearer sk-..."}
}'
```

### Response

Body is streamed as content chunks. First chunk includes metadata:

| Metadata key | Type | Description |
|---|---|---|
| `http-client:status` | u16 | HTTP status code |
| `http-client:headers` | HeaderMap | Response headers (CBOR-encoded) |

HTTP error status codes (4xx, 5xx) are returned normally — check `http-client:status`.

## Capabilities

Requires `wasi:http` — declared in `act.toml`.

## License

MIT OR Apache-2.0
