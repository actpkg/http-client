# http-client

ACT component that makes outbound HTTP requests via `wasi:http/client`.

## Tools

### `fetch`

Make an HTTP request.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | URL to fetch |
| `method` | string | no | HTTP method (default `GET`). One of: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`, `QUERY` |
| `headers` | object | no | Request headers as key-value pairs |
| `body` | string | no | Request body |

Returns JSON with `status` (number) and `body` (string).

## Build

```sh
cargo build --target wasm32-wasip2 --release
```

The component is at `target/wasm32-wasip2/release/http_client.wasm`.

## Usage

```sh
# CLI
act-host call http_client.wasm fetch --args '{"url": "https://httpbin.org/get"}'

# POST with headers and body
act-host call http_client.wasm fetch --args '{
  "url": "https://httpbin.org/post",
  "method": "POST",
  "headers": {"Content-Type": "application/json"},
  "body": "{\"key\": \"value\"}"
}'

# HTTP server
act-host serve http_client.wasm
curl -X POST http://localhost:3000/tools/fetch \
  -H 'Content-Type: application/json' \
  -d '{"arguments": {"url": "https://httpbin.org/get"}}'
```

## Capabilities

Requires `wasi:http/client` — the host must provide outbound HTTP access.
