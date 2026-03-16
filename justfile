wasm := "target/wasm32-wasip2/release/http_client.wasm"
act := env("ACT", "act")
port := "3456"
addr := "[::1]:" + port

build:
    cargo build --target wasm32-wasip2 --release

test: build
    #!/usr/bin/env bash
    {{act}} serve {{wasm}} --listen "{{addr}}" &
    trap "kill $!" EXIT
    npx wait-on http://[::1]:{{port}}/info
    hurl --test --variable "port={{port}}" e2e/*.hurl
