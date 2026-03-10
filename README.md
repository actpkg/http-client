# http-client

ACT component that makes outbound HTTP requests via `wasi:http/client`.

## Tools

### `fetch`

Make an HTTP request.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `url` | string | yes | | URL to fetch |
| `method` | string | no | `GET` | HTTP method — any valid method including WebDAV (`PROPFIND`, etc.) |
| `headers` | object | no | `{}` | Request headers as key-value pairs |
| `body` | string | no | | Text request body (UTF-8) |
| `body_json` | any | no | | JSON request body. Auto-serialized, auto-sets `Content-Type: application/json` |
| `body_raw` | bytes | no | | Raw binary request body (CBOR byte string) |
| `timeout_ms` | integer | no | | Request timeout in milliseconds |
| `follow_redirects` | boolean | no | `true` | Whether to follow HTTP redirects (up to 10 hops) |

Body priority: `body_raw` > `body_json` > `body`. Only one is used per request.

**Response:** The response body is streamed as content chunks with the original `Content-Type`. The first chunk carries component-specific metadata:

| Metadata key | Type | Description |
|--------------|------|-------------|
| `http-client:status` | u16 | HTTP status code |
| `http-client:headers` | HeaderMap | Response headers (CBOR-encoded via `http-serde`) |
| `http-client:trailers` | HeaderMap | HTTP trailers, if present (sent as a final metadata-only chunk) |

## Build

```sh
cargo build --target wasm32-wasip2 --release
```

The component is at `target/wasm32-wasip2/release/http_client.wasm`.

## Usage

```sh
# Simple GET
act-host call http_client.wasm fetch --args '{"url": "https://httpbin.org/get"}'

# POST with JSON body (Content-Type auto-set)
act-host call http_client.wasm fetch --args '{
  "url": "https://httpbin.org/post",
  "method": "POST",
  "body_json": {"key": "value"}
}'

# Custom headers
act-host call http_client.wasm fetch --args '{
  "url": "https://api.example.com/data",
  "headers": {"Authorization": "Bearer token123"}
}'

# Timeout
act-host call http_client.wasm fetch --args '{
  "url": "https://slow-api.example.com",
  "timeout_ms": 5000
}'

# HTTP server mode
act-host serve http_client.wasm
curl -X POST http://localhost:3000/tools/fetch \
  -H 'Content-Type: application/json' \
  -d '{"id": "1", "arguments": {"url": "https://httpbin.org/get"}}'
```

## Capabilities

Requires `wasi:http/client` — the host must provide outbound HTTP access.
