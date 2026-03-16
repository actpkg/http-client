GET {{baseurl}}/info
HTTP 200
[Asserts]
jsonpath "$['std:name']" == "http-client"
jsonpath "$['std:version']" isString
