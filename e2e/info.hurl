GET {{host}}/info
HTTP 200
[Asserts]
jsonpath "$.name" == "http-client"
jsonpath "$.version" isString
