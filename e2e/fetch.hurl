# Basic GET request (fetch the host's own /info endpoint)
# Response data is base64-encoded since mime_type is application/json
POST http://[::1]:{{port}}/tools/fetch
{"arguments": {"url": "http://[::1]:{{port}}/info"}}
HTTP 200
[Asserts]
jsonpath "$.content[0].mime_type" == "application/json"

# GET with custom header
POST http://[::1]:{{port}}/tools/fetch
{"arguments": {"url": "http://[::1]:{{port}}/info", "headers": {"X-Test": "hello"}}}
HTTP 200
[Asserts]
jsonpath "$.content[0].mime_type" == "application/json"

# Invalid URL (no scheme)
POST http://[::1]:{{port}}/tools/fetch
{"arguments": {"url": "not-a-url"}}
HTTP 422
[Asserts]
jsonpath "$.error.kind" == "std:invalid-args"
