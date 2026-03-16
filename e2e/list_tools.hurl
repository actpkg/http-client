POST http://[::1]:{{port}}/tools
HTTP 200
[Asserts]
jsonpath "$.tools" count >= 1
jsonpath "$.tools[*].name" contains "fetch"
