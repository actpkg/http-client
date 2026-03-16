GET http://[::1]:{{port}}/info
HTTP 200
[Asserts]
jsonpath "$['std:name']" == "http-client"
jsonpath "$['std:version']" isString
