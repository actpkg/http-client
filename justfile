wasm := "target/wasm32-wasip2/release/http_client.wasm"
act := env("ACT", "act")
port := `python3 -c 'import socket; s=socket.socket(socket.AF_INET, socket.SOCK_STREAM); s.bind(("", 0)); print(s.getsockname()[1]); s.close()'`
addr := "[::1]:" + port
baseurl := "http://" + addr

init:
    wit-deps

setup: init
    prek install

build:
    cargo build --release

test:
    #!/usr/bin/env bash
    {{act}} run {{wasm}} --listen "{{addr}}" &
    trap "kill $!" EXIT
    npx wait-on {{baseurl}}/info
    hurl --test --variable "baseurl={{baseurl}}" e2e/*.hurl
