---
name: http-client
description: Make HTTP/HTTPS requests — fetch URLs, call APIs, download content
metadata:
  act: {}
---

# HTTP Client Component

Make outbound HTTP/HTTPS requests from a sandboxed WASM component.

## Tool: fetch

Single tool that handles all HTTP methods, body types, headers, timeouts, and redirects. Response body is streamed; status code and headers are returned as metadata on the first content chunk.

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `url` | string | required | URL to fetch |
| `method` | string | `"GET"` | HTTP method (GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS) |
| `headers` | object | `{}` | Request headers as key-value pairs |
| `body` | string | — | Text request body (UTF-8) |
| `body_json` | any | — | JSON request body (auto-serialized, auto-sets Content-Type) |
| `body_raw` | bytes | — | Raw binary request body |
| `timeout_ms` | number | — | Request timeout in milliseconds |
| `follow_redirects` | boolean | `true` | Whether to follow HTTP redirects (up to 10 hops) |

Only one body type can be provided: `body`, `body_json`, or `body_raw`.

### Examples

**Simple GET:**
```
fetch(url: "https://httpbin.org/get")
```

**POST JSON to an API:**
```
fetch(
  url: "https://api.example.com/data",
  method: "POST",
  body_json: {"key": "value", "count": 42},
  headers: {"Authorization": "Bearer sk-..."}
)
```

**Download with timeout:**
```
fetch(
  url: "https://example.com/large-file.zip",
  timeout_ms: 30000
)
```

**PUT with custom headers:**
```
fetch(
  url: "https://api.example.com/resource/123",
  method: "PUT",
  body: "updated content",
  headers: {"Content-Type": "text/plain", "If-Match": "etag-value"}
)
```

**HEAD request (check resource without downloading):**
```
fetch(url: "https://example.com/file.pdf", method: "HEAD")
```

### Response

The response body is streamed as content chunks. The first chunk includes metadata:

- `http-client:status` — HTTP status code (e.g. 200, 404)
- `http-client:headers` — Response headers as a map

For text responses (HTML, JSON, etc.), content is returned as a string. For binary responses, content is returned as raw bytes with the appropriate MIME type.

### Error Handling

- Invalid URL → `std:invalid-args`
- Network/timeout/DNS errors → `std:internal`
- HTTP error status codes (4xx, 5xx) are NOT errors — they return normally with the status code in metadata. Check `http-client:status` to distinguish success from failure.
