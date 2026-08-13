"""Shared fixtures for the MCP-driven e2e suite.

The suite drives the packed component through `act run --mcp` over stdio with
a real MCP client, so what the tests observe is what an agent observes.

The old ACT-HTTP suite never depended on the live network either: its
`fetch` tests targeted the same host's own `/info` endpoint (`{{baseurl}}
/info`) — a self-fetch that only worked because ACT-HTTP was already
listening as the transport under test. MCP stdio has no listener to
self-fetch, so this suite starts a tiny local stub HTTP server instead (see
`stub_server` below) — same property (no live/public endpoint, nothing to
go dark in CI), different mechanism.
"""

import asyncio
import http.server
import json
import os
import shlex
import socket
import subprocess
import threading
import time
import pytest
from contextlib import AsyncExitStack
from pathlib import Path

from fastmcp import Client
from fastmcp.client.transports import StdioTransport

# Measured in docs/specs/2026-08-08-e2e-harness-findings.md, question 1.
from mcp.shared.exceptions import McpError

WASM = "target/wasm32-wasip2/release/component_http_client.wasm"

# ACT's audit trail writes to stderr unconditionally — it is not governed by
# RUST_LOG — so it is redirected to a file rather than left to flood pytest.
LOG_FILE = Path(".pytest-act-stderr.log")

# Deliberately loose. `act run --mcp` instantiates the component before it
# answers `initialize`, so "connect" includes that cost -- for a heavy
# component (servo embeds a browser engine) it is seconds, and on a loaded
# runner it varies. 30s tripped servo in CI while its healthy connect was
# ~8s, so the bound sits well above the worst observed cost and still well
# below the per-test timeout, keeping this the diagnostic that fires first.
CONNECT_TIMEOUT = 120


@pytest.fixture(scope="session")
def act_command() -> list[str]:
    """The ACT invocation, honouring the same override the justfile uses.

    Parsed with shlex, not treated as a single path: the justfile's own
    default for its `act` variable is `npx @actcore/act` — two words — which
    cannot be `argv[0]` for a non-shell `subprocess.run`/`StdioTransport`
    call. A bare `os.environ.get("ACT", "act")` string breaks that default;
    splitting it is what makes both forms ("act" on PATH, and the npx
    two-word default) actually spawn.
    """
    return shlex.split(os.environ.get("ACT", "act"))


@pytest.fixture(scope="session")
def wasm_path(act_command: list[str]) -> Path:
    """The packed component.

    Existence is not enough and neither is a fresh mtime: `cargo build`
    produces a wasm with no `act:component` custom section, and an unpacked
    artifact declares no capability ceiling, so every grant is refused as
    "outside ceiling" and the failures point anywhere but here. This has
    already bitten this workspace repeatedly, so the fixture checks the
    section rather than the file.
    """
    path = Path(WASM)
    if not path.exists():
        pytest.fail(f"{path} is missing — run `just build && just pack` first")
    probe = subprocess.run(
        [*act_command, "inspect", "component-manifest", str(path)],
        capture_output=True, text=True,
    )
    name = json.loads(probe.stdout or "{}").get("std", {}).get("name", "unknown")
    if name in ("", "unknown"):
        pytest.fail(f"{path} is built but not packed — run `just pack`")
    return path


@pytest.fixture
async def client(act_command: list[str], wasm_path: Path):
    """A connected MCP client, one `act` process per test.

    `--allow wasi:http` moved here verbatim from the old justfile — the
    component's own ceiling is `host = "*"` (act.toml: "Host scope is
    delegated to the host policy"), so opening the class grants exactly what
    the old ACT-HTTP recipe granted, nothing wider.
    """
    transport = StdioTransport(
        command=act_command[0],
        args=[*act_command[1:], "run", str(wasm_path), "--mcp", "--allow", "wasi:http"],
        keep_alive=False,
        log_file=LOG_FILE,
    )
    async with AsyncExitStack() as stack:
        # Bound the connect, not the test body. A stalled handshake otherwise
        # consumes the whole pytest timeout with no diagnostic at all — which
        # is precisely how the webdriver-bidi CI hang presented for hours.
        try:
            async with asyncio.timeout(CONNECT_TIMEOUT):
                connected = await stack.enter_async_context(Client(transport))
        except TimeoutError:
            pytest.fail(
                f"MCP client did not connect within {CONNECT_TIMEOUT}s; "
                f"act's stderr, if it wrote any, is dumped at session end"
            )
        yield connected


class _StubHandler(http.server.BaseHTTPRequestHandler):
    """Echoes the request path and headers back as JSON — enough for `fetch`
    to have something real to GET, and enough for a test to prove a custom
    header actually reached the server, not just that the call succeeded."""

    def do_GET(self):
        body = json.dumps({
            "path": self.path,
            "headers": {k.lower(): v for k, v in self.headers.items()},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass  # keep pytest -v output readable


def _port_open(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(0.2)
        return s.connect_ex(("127.0.0.1", port)) == 0


@pytest.fixture
def stub_server():
    """A local HTTP server for `fetch` to target, in a background thread —
    no subprocess, no node/docker dependency, nothing to go stale.

    `HTTPServer.__init__` binds and listens synchronously before this
    fixture ever starts the serving thread, so in practice the socket is
    already accepting connections here — but a thread that hasn't been
    scheduled yet is still a race in principle, and starting a stub and
    hoping cost another component in this migration a red CI run. Waited on
    explicitly rather than trusted.
    """
    server = http.server.HTTPServer(("127.0.0.1", 0), _StubHandler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    for _ in range(50):
        if _port_open(port):
            break
        time.sleep(0.05)
    else:
        server.shutdown()
        pytest.fail(f"stub server did not open port {port} in time")

    yield f"http://127.0.0.1:{port}"

    server.shutdown()
    thread.join(timeout=5)


@pytest.fixture
def expect_error():
    """Assert a call fails with a specific ACT error kind (and, optionally, a
    substring of its human-readable message).

    Exposed as a fixture rather than a plain function so tests never have to
    import from `conftest` — that import only resolves when the test
    directory happens to be on `sys.path`, which is not something to rely on.

    Measured, not assumed. `call-tool` in `act:tools` returns a bare
    `tool-result` with NO `result<>` wrapper — only `list-tools` has one — so
    a guest reporting a failed tool call can only do it through
    `tool-event::error`, which arrives as a result with `is_error` set and the
    kind in `_meta`, and the message as its one text content part. **That is
    the path `fetch`'s own URL-parsing error takes.**

    The JSON-RPC error path exists for failures that are not the guest's tool
    body: `list-tools`, the session operations, a wasmtime trap, an
    unreachable actor. It raises `mcp.shared.exceptions.McpError`, with the
    kind at `exc.error.data` and the message at `exc.error.message`. Both are
    handled here so callers need not care which one fires; this component
    has no session-provider tools that would reach it, but the shape is kept
    for consistency with the rest of this migration.
    """

    async def _expect(
        client, tool: str, arguments: dict, kind: str,
        contains: str | None = None, meta: dict | None = None,
    ):
        try:
            result = await client.call_tool(tool, arguments, meta=meta, raise_on_error=False)
        except McpError as exc:
            data = getattr(getattr(exc, "error", None), "data", None) or {}
            assert data.get("dev.actcore/error-kind") == kind, (
                f"expected {kind} on the JSON-RPC error path, got {data!r}"
            )
            if contains is not None:
                message = getattr(exc.error, "message", "") or ""
                assert contains in message, f"expected {contains!r} in {message!r}"
            return

        assert result.is_error, f"expected {tool} to fail, got {result!r}"
        result_meta = result.meta or {}
        assert result_meta.get("dev.actcore/error-kind") == kind, (
            f"expected {kind} on the isError path, got {result_meta!r}"
        )
        if contains is not None:
            message = result.content[0].text if result.content else ""
            assert contains in message, f"expected {contains!r} in {message!r}"

    return _expect


def pytest_sessionfinish(session, exitstatus):
    """Print act's stderr when the run did not pass.

    `log_file` keeps the audit trail out of the test output, which is right
    for a green run and wrong for every other kind: on an ephemeral CI runner
    nothing ever reads that file. Diagnosing a CI-only hang in this fleet
    cost several rounds of probing that one line of this stream would have
    answered. A hook rather than a fixture finaliser on purpose — fixture
    teardown does not run when the session dies mid-test.
    """
    if exitstatus == 0 or not LOG_FILE.exists():
        return
    text = LOG_FILE.read_text(errors="replace").strip()
    if text:
        print(f"\n--- act stderr ({LOG_FILE}) ---\n{text}")
