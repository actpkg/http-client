//! Drive the packed component through `act run --mcp` with a real MCP client.
//!
//! This replaces the python fastmcp/pytest suite that used to live in this
//! directory: the tests observe exactly what an agent observes, over the same
//! client stack (`rmcp`) the host bridge itself is built on.
//!
//! Env: WASM — path to the packed component (default: the component's
//!      release build output);
//!      ACT  — the act invocation (default `act`; `npx @actcore/act`, the
//!             component justfile's default, also works — whitespace-split,
//!             like the shlex.split the python conftest did).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

/// `().serve(transport)` hands back the client-role service running over the
/// child process: role first, the unit client handler second.
type Client = rmcp::service::RunningService<rmcp::service::RoleClient, ()>;

fn wasm_path() -> PathBuf {
    PathBuf::from(std::env::var("WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/wasm32-wasip2/release/component_http_client.wasm"
        )
        .into()
    }))
}

/// The ACT invocation, honouring the same override the component justfile
/// uses. Its default there is `npx @actcore/act` — two words — which cannot
/// be `argv[0]` for a non-shell spawn, so the value is whitespace-split into
/// program + leading args. Quoted paths with spaces are not a form this
/// fleet passes through `ACT`; a full shlex is deliberately not pulled in.
fn act_argv() -> Vec<String> {
    std::env::var("ACT")
        .unwrap_or_else(|_| "act".into())
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Spawn `act run <wasm> --mcp` with the grant this component needs.
///
/// Grants are NOT optional: the default policy mode is `ask` and a headless
/// run degrades it to deny. The component's ceiling is `host = "*"`
/// (act.toml: "Host scope is delegated to the host policy"), so opening the
/// `wasi:http` class grants exactly what the python conftest granted,
/// nothing wider.
fn act_command() -> tokio::process::Command {
    let argv = act_argv();
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.arg("run").arg(wasm_path()).arg("--mcp");
    cmd.args(["--allow", "wasi:http"]);
    cmd
}

fn spawn_transport() -> TokioChildProcess {
    TokioChildProcess::new(act_command()).expect("spawn act run --mcp")
}

/// Spawn with stderr captured: the audit trail (refusals, per-call rollup)
/// writes there unconditionally — RUST_LOG never silences it.
fn spawn_with_captured_stderr() -> (TokioChildProcess, Arc<AsyncMutex<String>>) {
    let (transport, stderr) = TokioChildProcess::builder(act_command())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn act run --mcp with piped stderr");

    let captured = Arc::new(AsyncMutex::new(String::new()));
    let sink = captured.clone();
    let mut lines = BufReader::new(stderr.expect("stderr was piped")).lines();
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            sink.lock().await.push_str(&line);
            sink.lock().await.push('\n');
        }
    });

    (transport, captured)
}

/// Poll the captured stderr until `needle` appears — the audit line is
/// flushed before the JSON-RPC reply, but reaching this buffer still crosses
/// a pipe and an async read.
async fn wait_for_stderr(
    captured: &Arc<AsyncMutex<String>>,
    needle: &str,
    timeout: Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        if captured.lock().await.contains(needle) {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn connect() -> Client {
    ().serve(spawn_transport())
        .await
        .expect("rmcp handshake with act run --mcp")
}

fn text_blocks(result: &rmcp::model::CallToolResult) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|b| match b {
            rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

fn first_text_block(result: &rmcp::model::CallToolResult) -> &rmcp::model::TextContent {
    match result.content.first() {
        Some(rmcp::model::ContentBlock::Text(t)) => t,
        other => panic!("expected the first content block to be Text, got: {other:?}"),
    }
}

/// The kind and message of a failed call may arrive on either path: as a
/// JSON-RPC error response (`ErrorData.data` / `message`) or as an isError
/// result (`_meta` / text content). The python conftest's `expect_error`
/// fixture handled both; so does this. `call-tool` has no `result<>`
/// wrapper, so a guest reporting a failed call can only do it through
/// `tool-event::error` — which is the isError path here; the JSON-RPC path
/// stays handled for the non-guest failure modes.
async fn error_kind_of(client: &Client, params: CallToolRequestParams) -> Option<(String, String)> {
    match client.call_tool(params).await {
        Err(rmcp::ServiceError::McpError(e)) => {
            let kind = e
                .data
                .as_ref()
                .and_then(|d| d.get("dev.actcore/error-kind"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            kind.map(|k| (k, e.message.to_string()))
        }
        Ok(result) => {
            assert_eq!(result.is_error, Some(true), "call must fail: {result:?}");
            let kind = result
                .meta
                .as_ref()
                .and_then(|m| m.0.get("dev.actcore/error-kind"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let message = result
                .content
                .first()
                .and_then(|b| match b {
                    rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            kind.map(|k| (k, message))
        }
        Err(other) => panic!("unexpected transport failure: {other:?}"),
    }
}

/// A local HTTP server for `fetch` to target — the rust analogue of the
/// python conftest's `stub_server`. It echoes the request path and
/// lowercased headers back as JSON: enough for `fetch` to have something
/// real to GET, and enough for a test to prove a custom header actually
/// reached the server, not just that the call succeeded. Bound on an
/// ephemeral port, served from a detached task — no live/public endpoint,
/// nothing to go dark in CI.
async fn spawn_stub_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub server");
    let addr = listener.local_addr().expect("stub server addr");
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                // A GET has no body: end-of-headers is end-of-request.
                let mut buf = Vec::with_capacity(1024);
                let mut chunk = [0u8; 1024];
                loop {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buf);
                let mut lines = text.split("\r\n");
                let request_line = lines.next().unwrap_or("");
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                let mut headers = serde_json::Map::new();
                for line in lines.by_ref() {
                    if line.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        headers.insert(
                            k.trim().to_ascii_lowercase(),
                            Value::String(v.trim().to_string()),
                        );
                    }
                }
                let body = serde_json::to_vec(&json!({"path": path, "headers": headers}))
                    .expect("stub echo serializes");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

/// The manifest probe from the python test_info.py: the packed artifact
/// must declare its name and a version. Also the fast-fail the python
/// `wasm_path` fixture provided — an unpacked wasm (raw `cargo build`
/// output, no `act:component` section) declares no ceiling, every grant is
/// refused as "outside ceiling", and the failures point anywhere but at the
/// missing metadata. The justfile's `test: build` ordering exists so this
/// test finds a packed artifact.
#[test]
fn manifest_reports_name_and_version() {
    let output = {
        let argv = act_argv();
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.args(["inspect", "component-manifest"])
            .arg(wasm_path())
            .output()
            .expect("run act inspect component-manifest")
    };
    assert!(
        output.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: Value = serde_json::from_slice(&output.stdout).expect("manifest is JSON");
    assert_eq!(
        manifest["std"]["name"], "http-client",
        "packed manifest must carry the component name"
    );
    assert!(
        manifest["std"]["version"].is_string(),
        "packed manifest must carry a version, got: {}",
        manifest["std"]["version"]
    );
}

#[tokio::test]
async fn component_exposes_the_fetch_tool() {
    let client = connect().await;
    let tools = client.list_all_tools().await.expect("list_all_tools");
    assert!(
        tools.iter().any(|t| t.name == "fetch"),
        "fetch must be among the tools, got: {:?}",
        tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>()
    );
    client.cancel().await.ok();
}

#[tokio::test]
async fn fetch_returns_json_with_status_meta() {
    let client = connect().await;
    let stub = spawn_stub_server().await;

    let args = json!({ "url": stub }).as_object().unwrap().clone();
    let result = client
        .call_tool(CallToolRequestParams::new("fetch").with_arguments(args))
        .await
        .expect("call_tool fetch");
    assert_ne!(result.is_error, Some(true), "fetch failed: {result:?}");

    let block = first_text_block(&result);
    let meta = block
        .meta
        .as_ref()
        .expect("first text block must carry _meta");
    assert_eq!(
        meta.0.get("dev.actcore/mime-type").and_then(|v| v.as_str()),
        Some("application/json"),
        "the stub's Content-Type must surface as the block mime-type"
    );
    assert_eq!(
        meta.0.get("http-client:status").and_then(Value::as_i64),
        Some(200),
        "fetch reports the upstream status via metadata (src/lib.rs sends it \
         as http-client:status); the python suite checked it as a number"
    );

    let echoed: Value =
        serde_json::from_str(&block.text).expect("the stub's echo body is JSON");
    assert_eq!(echoed["path"], "/", "a bare stub URL GETs /");

    client.cancel().await.ok();
}

#[tokio::test]
async fn fetch_sends_custom_headers() {
    let client = connect().await;
    let stub = spawn_stub_server().await;

    let args = json!({
        "url": stub,
        "headers": {"X-Test": "hello"},
    })
    .as_object()
    .unwrap()
    .clone();
    let result = client
        .call_tool(CallToolRequestParams::new("fetch").with_arguments(args))
        .await
        .expect("call_tool fetch");
    assert_ne!(result.is_error, Some(true), "fetch failed: {result:?}");

    // The stub echoes request headers back, so this checks the thing the
    // test name claims to check — the header reached the server. (The old
    // python assertion compared the mime-type, identical to the plain-GET
    // case, and proved nothing about the header.)
    let echoed: Value =
        serde_json::from_str(&first_text_block(&result).text).expect("echo body is JSON");
    assert_eq!(
        echoed["headers"]["x-test"], "hello",
        "X-Test must reach the stub, lowercased, in: {echoed}"
    );

    client.cancel().await.ok();
}

#[tokio::test]
async fn fetch_rejects_a_url_with_no_scheme() {
    let client = connect().await;

    // The host validates args against the tool schema BEFORE the guest runs
    // (ACT-SPEC §6.4), but a schema-valid url that is not a URL trips the
    // guest's send: hclient files it under ErrorKind::Uri and the component
    // maps that to std:invalid-args. The message is hclient's own wording —
    // the wasi-fetch era's "Missing URL scheme" (and the http::Uri pre-check
    // that briefly reimplemented it) are gone; the pre-check also blocked
    // IDN hosts, which the idn feature punycodes.
    let args = json!({ "url": "not-a-url" }).as_object().unwrap().clone();
    let (kind, message) = error_kind_of(
        &client,
        CallToolRequestParams::new("fetch").with_arguments(args),
    )
    .await
    .expect("the failure must carry a named error kind");
    assert_eq!(
        kind, "std:invalid-args",
        "the URL-parsing failure must surface as std:invalid-args"
    );
    assert!(
        message.contains("no scheme"),
        "expected hclient's scheme-less message, got: {message}"
    );

    client.cancel().await.ok();
}

/// Beyond python parity: the audit machinery. A fetch call must leave its
/// rollup line on stderr, and the captured-stderr plumbing this harness
/// uses for refusals must actually see it.
#[tokio::test]
async fn fetch_call_is_audited() {
    let (transport, captured) = spawn_with_captured_stderr();
    let client = ().serve(transport).await.expect("rmcp handshake");
    let stub = spawn_stub_server().await;

    let args = json!({ "url": stub }).as_object().unwrap().clone();
    let result = client
        .call_tool(CallToolRequestParams::new("fetch").with_arguments(args))
        .await
        .expect("call_tool fetch");
    assert_ne!(result.is_error, Some(true), "fetch failed: {result:?}");

    assert!(
        wait_for_stderr(&captured, "req:", Duration::from_secs(5)).await,
        "expected a per-call rollup line in the audit trail:\n{}",
        captured.lock().await
    );

    client.cancel().await.ok();
}
