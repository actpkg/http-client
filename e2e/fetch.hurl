# Basic GET request (fetch the host's own /info endpoint)
# Response data is base64-encoded since mime_type is application/json
POST {{baseurl}}/tools/fetch
{"arguments": {"url": "{{baseurl}}/info"}}
HTTP 200
[Asserts]
jsonpath "$.content[0].mime_type" == "application/json"

# GET with custom header
POST {{baseurl}}/tools/fetch
{"arguments": {"url": "{{baseurl}}/info", "headers": {"X-Test": "hello"}}}
HTTP 200
[Asserts]
jsonpath "$.content[0].mime_type" == "application/json"

# Invalid URL (no scheme)
POST {{baseurl}}/tools/fetch
{"arguments": {"url": "not-a-url"}}
HTTP 422
[Asserts]
jsonpath "$.error.kind" == "std:invalid-args"
