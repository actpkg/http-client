GET {{baseurl}}/info
HTTP 200
[Asserts]
jsonpath "$.std.name" "http_client"
jsonpath "$.std.version" isString
