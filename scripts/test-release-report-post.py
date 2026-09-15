#!/usr/bin/env python3
"""Stub-endpoint proof for release-report-post.py."""

from __future__ import annotations

import base64
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / "scripts" / "release-report-post.py"
PDF = ROOT / "docs" / "release-reports" / "v3.19.0.pdf"
USER_THREAD = "1710000000.000001"
DEV_THREAD = "1710000000.000010"
OLD_THREAD = "1709999999.999999"
NEW_REPLY = "1710000000.000002"
READ_SINCE = "2024-03-09T16:00:00.000001Z"


class StubState:
    def __init__(self, pdf: bytes) -> None:
        self.pdf = pdf
        self.remote_pdf = pdf
        self.requests: list[dict] = []
        self.download_requests: list[str] = []
        self.include_file_block = True
        self.include_download_url = True
        self.read_mode = "valid"
        self.read_pdf = pdf
        self.read_since = None
        self.read_messages: list[str] = []


def envelope(result: dict) -> bytes:
    return json.dumps({"jsonrpc": "2.0", "id": result.pop("_id", 1), "result": result}).encode()


def handler_for(state: StubState):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format: str, *_args: object) -> None:
            return

        def do_GET(self) -> None:  # noqa: N802
            state.download_requests.append(self.path)
            assert self.headers.get("Authorization") == "Bearer stub-token"
            assert self.headers.get("x-vercel-protection-bypass") == "stub-bypass"
            if self.path != "/download/report.pdf":
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/pdf")
            self.send_header("Content-Length", str(len(state.remote_pdf)))
            self.end_headers()
            self.wfile.write(state.remote_pdf)

        def do_POST(self) -> None:  # noqa: N802
            assert self.headers.get("Authorization") == "Bearer stub-token"
            assert self.headers.get("x-vercel-protection-bypass") == "stub-bypass"
            size = int(self.headers.get("Content-Length", "0"))
            request = json.loads(self.rfile.read(size))
            state.requests.append(request)
            method = request.get("method")
            request_id = request.get("id", 1)
            if method == "initialize":
                body = envelope(
                    {
                        "_id": request_id,
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "serverInfo": {"name": "stub", "version": "1"},
                    }
                )
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Mcp-Session-Id", "stub-session")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if method == "notifications/initialized":
                self.send_response(202)
                self.end_headers()
                return
            if method == "tools/list":
                body = envelope(
                    {
                        "_id": request_id,
                        "tools": [{"name": "mecha_read"}, {"name": "mecha_post"}],
                    }
                )
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if method == "tools/call":
                tool_name = request["params"]["name"]
                if tool_name == "mecha_read":
                    arguments = request["params"]["arguments"]
                    assert arguments["channel"] == "cas-internal"
                    assert arguments["since"] == READ_SINCE
                    assert arguments["include_files"] is True
                    assert arguments["include_threads"] is True
                    assert arguments["max_file_bytes"] == 4 * 1024 * 1024
                    assert arguments["max_files"] == 50
                    state.read_since = arguments["since"]
                    # The hub's bounded response excludes older roots while
                    # retaining the selected User root and its reply.
                    state.read_messages = [USER_THREAD, NEW_REPLY]
                    read_file = {
                        "file_id": "F-report",
                        "size_bytes": len(state.read_pdf),
                    }
                    if state.read_mode == "wrong_file_id":
                        read_file["file_id"] = "F-other"
                    if state.read_mode == "invalid_base64":
                        read_file["content_base64"] = "not-base64"
                    elif state.read_mode != "missing_base64":
                        read_file["content_base64"] = base64.b64encode(state.read_pdf).decode("ascii")
                    if state.read_mode == "size_mismatch":
                        read_file["size_bytes"] += 1
                    result = {
                        "_id": request_id,
                        "content": [
                            {
                                "type": "text",
                                "text": json.dumps(
                                    {
                                        "messages": [
                                            {
                                                "message_id": USER_THREAD,
                                                "thread_id": USER_THREAD,
                                                "parent_message_id": None,
                                            },
                                            {
                                                "message_id": NEW_REPLY,
                                                "thread_id": USER_THREAD,
                                                "parent_message_id": USER_THREAD,
                                            },
                                        ],
                                        "files": [
                                            read_file
                                        ]
                                    }
                                ),
                            }
                        ],
                    }
                    body = envelope(result)
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                arguments = request["params"]["arguments"]
                assert tool_name == "mecha_post"
                if arguments["kind"] == "file":
                    assert arguments["reply_to"] == USER_THREAD
                    assert arguments["file"]["content_encoding"] == "base64"
                    assert base64.b64decode(arguments["file"]["content"]) == state.pdf
                    file_receipt = {
                        "file_id": "F-report",
                        "permalink": "https://example.test/files/report.pdf",
                    }
                    if state.include_download_url:
                        file_receipt["download_url"] = (
                            f"http://127.0.0.1:{self.server.server_port}/download/report.pdf"
                        )
                    receipt = {
                        "ok": True,
                        "schema_version": 1,
                        "kind": "file",
                        "message": {
                            "message_id": "file-message",
                            "thread_id": USER_THREAD,
                            "permalink": "https://example.test/files/report.pdf",
                        },
                    }
                    if state.include_file_block:
                        receipt["file"] = file_receipt
                    result = {
                        "_id": request_id,
                        "content": [
                            {
                                "type": "text",
                                "text": json.dumps(receipt),
                            }
                        ],
                    }
                else:
                    assert arguments["kind"] == "message"
                    assert arguments["reply_to"] == DEV_THREAD
                    assert "https://github.com/Richards-LLC/cassy/blob/main/docs/release-reports/v9.99.0.html" in arguments["text"]
                    result = {
                        "_id": request_id,
                        "content": [
                            {
                                "type": "text",
                                "text": json.dumps(
                                    {
                                        "ok": True,
                                        "schema_version": 1,
                                        "kind": "message",
                                        "message": {
                                            "message_id": "html-message",
                                            "thread_id": DEV_THREAD,
                                            "permalink": "https://example.test/messages/html",
                                        },
                                    }
                                ),
                            }
                        ],
                    }
                body = envelope(result)
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            self.send_error(400, f"unexpected method {method}")

    return Handler


class FakeResponse:
    def __init__(self, status: int, body: bytes = b"", headers: dict[str, str] | None = None) -> None:
        self.status = status
        self.body = body
        self.headers = headers or {}
        self.closed = False

    def read(self, _limit: int = -1) -> bytes:
        return self.body

    def getcode(self) -> int:
        return self.status

    def close(self) -> None:
        self.closed = True


class FakeOpener:
    def __init__(self, responses: list[FakeResponse]) -> None:
        self.responses = responses
        self.requests: list[urllib.request.Request] = []

    def open(self, request: urllib.request.Request, timeout: float):
        del timeout
        self.requests.append(request)
        if not self.responses:
            raise AssertionError("unexpected request reached the fake sink")
        return self.responses.pop(0)


def adapter_module():
    spec = importlib.util.spec_from_file_location("release_report_post", ADAPTER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def request_headers(request: urllib.request.Request) -> dict[str, str]:
    return {key.lower(): value for key, value in request.header_items()}


def expect_adapter_error(module, action, label: str) -> None:
    try:
        action()
    except module.AdapterError:
        return
    raise AssertionError(f"{label} must fail closed")


def run_download_policy_proof() -> None:
    """Exercise production download auth and redirect policy with fake sinks."""

    module = adapter_module()
    client = module.McpClient("https://hub.example.test/mcp", "stub-token", "stub-bypass", 1)
    external = FakeOpener([FakeResponse(200, b"signed-pdf")])
    client.opener = external
    assert client.download("https://signed.example.test/report.pdf") == b"signed-pdf"
    headers = request_headers(external.requests[0])
    assert "authorization" not in headers
    assert "x-vercel-protection-bypass" not in headers

    downgrade = FakeOpener([])
    client.opener = downgrade
    expect_adapter_error(
        module,
        lambda: client.download("http://signed.example.test/report.pdf"),
        "plaintext external PDF URL",
    )
    assert downgrade.requests == []

    external_redirect = FakeOpener(
        [FakeResponse(302, headers={"Location": "https://sink.example.test/report.pdf"})]
    )
    client.opener = external_redirect
    expect_adapter_error(
        module,
        lambda: client.download("https://signed.example.test/report.pdf"),
        "cross-origin signed URL redirect",
    )
    assert len(external_redirect.requests) == 1
    assert "authorization" not in request_headers(external_redirect.requests[0])

    authenticated_redirect = FakeOpener(
        [FakeResponse(302, headers={"Location": "https://sink.example.test/report.pdf"})]
    )
    client.opener = authenticated_redirect
    expect_adapter_error(
        module,
        lambda: client.download("https://hub.example.test/files/report.pdf"),
        "cross-origin authenticated redirect",
    )
    assert len(authenticated_redirect.requests) == 1
    assert request_headers(authenticated_redirect.requests[0])["authorization"].startswith(
        "Bearer "
    )

    downgrade_redirect = FakeOpener(
        [FakeResponse(302, headers={"Location": "http://hub.example.test/report.pdf"})]
    )
    client.opener = downgrade_redirect
    expect_adapter_error(
        module,
        lambda: client.download("https://hub.example.test/files/report.pdf"),
        "downgrade redirect",
    )
    assert len(downgrade_redirect.requests) == 1
    print("release-report-post transport policy: 5 boundary cases passed")


def run_adapter(
    server: ThreadingHTTPServer,
    state: StubState,
    remote_pdf: bytes,
    user_thread: str = USER_THREAD,
    dev_thread: str = DEV_THREAD,
):
    state.remote_pdf = remote_pdf
    state.requests.clear()
    state.download_requests.clear()
    with tempfile.TemporaryDirectory(prefix="release-report-post-") as directory:
        root = Path(directory)
        html = root / "v9.99.0.html"
        html.write_text("<!doctype html><title>stub</title>\n", encoding="utf-8")
        credentials = root / "credentials.env"
        credentials.write_text(
            "export MECHA_SLACK_TOKEN_TEST='stub-token'\n"
            "export MECHA_VERCEL_BYPASS='stub-bypass'\n",
            encoding="utf-8",
        )
        receipt = root / "release-report.receipt"
        environment = os.environ.copy()
        environment.update(
            {
                "CAS_RELEASE_TRAIN_REPORT_MCP_URL": f"http://127.0.0.1:{server.server_port}",
                "CAS_RELEASE_TRAIN_REPORT_RECEIPT": str(receipt),
                "CAS_RELEASE_TRAIN_REPORT_USER_THREAD_TS": user_thread,
                "CAS_RELEASE_TRAIN_REPORT_DEV_THREAD_TS": dev_thread,
                "CAS_CREDENTIALS_FILE": str(credentials),
                "CAS_RELEASE_TRAIN_MECHA_TOKEN_ENV": "MECHA_SLACK_TOKEN_TEST",
                "CAS_RELEASE_TRAIN_REPORT_REPO": "Richards-LLC/cassy",
            }
        )
        environment.pop("MECHA_SLACK_TOKEN_TEST", None)
        environment.pop("MECHA_VERCEL_BYPASS", None)
        result = subprocess.run(
            [sys.executable, str(ADAPTER), "v9.99.0", str(PDF), str(html), user_thread, dev_thread],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        fields = None
        if receipt.is_file():
            fields = dict(line.split("=", 1) for line in receipt.read_text(encoding="utf-8").splitlines())
        return result, fields


def main() -> int:
    pdf = PDF.read_bytes()
    run_download_policy_proof()
    state = StubState(pdf)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(state))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        result, fields = run_adapter(server, state, pdf)
        if result.returncode != 0:
            raise AssertionError(f"adapter failed: stdout={result.stdout!r} stderr={result.stderr!r}")
        assert fields is not None
        assert fields["TAG"] == "v9.99.0"
        assert fields["PDF_FILE_ID"] == "F-report"
        assert fields["HTML_FILE_ID"] == "html-message"
        assert fields["USER_THREAD_TS"] == USER_THREAD
        assert fields["DEV_THREAD_TS"] == DEV_THREAD
        assert len(fields["PDF_SHA256"]) == 64
        assert int(fields["PAGE_COUNT"]) > 0
        assert fields["PDF_SIZE_BYTES"] == str(len(pdf))
        assert fields["PDF_REMOTE_SHA256"] == fields["PDF_SHA256"]
        assert fields["PDF_REMOTE_SIZE_BYTES"] == fields["PDF_SIZE_BYTES"]
        assert fields["PDF_REMOTE_PAGE_COUNT"] == fields["PAGE_COUNT"]
        assert state.download_requests == ["/download/report.pdf"]
        methods = [request.get("method") for request in state.requests]
        assert methods == ["initialize", "notifications/initialized", "tools/list", "tools/call", "tools/call"]

        scenarios = {
            "truncated": pdf[:-17],
            "substituted": (ROOT / "docs" / "release-reports" / "v3.20.0.pdf").read_bytes(),
            "unreadable": b"not a PDF",
        }
        for label, remote_pdf in scenarios.items():
            result, fields = run_adapter(server, state, remote_pdf)
            assert result.returncode != 0, f"{label} remote bytes must fail closed"
            assert fields is None, f"{label} mismatch must not write a delivery receipt"
            assert state.download_requests == ["/download/report.pdf"]
            methods = [request.get("method") for request in state.requests]
            assert methods == ["initialize", "notifications/initialized", "tools/list", "tools/call"], label

        state.include_file_block = False
        state.include_download_url = False
        result, fields = run_adapter(server, state, pdf)
        assert result.returncode != 0
        assert "message permalink is not a PDF endpoint" in result.stderr
        assert fields is None
        assert state.download_requests == []
        methods = [request.get("method") for request in state.requests]
        assert methods == ["initialize", "notifications/initialized", "tools/list", "tools/call"]

        state.include_file_block = True
        result, fields = run_adapter(server, state, pdf)
        assert result.returncode == 0, f"hub file read fallback failed: {result.stderr!r}"
        assert fields is not None
        assert fields["PDF_REMOTE_SHA256"] == fields["PDF_SHA256"]
        assert fields["PDF_REMOTE_SIZE_BYTES"] == fields["PDF_SIZE_BYTES"]
        assert fields["PDF_REMOTE_PAGE_COUNT"] == fields["PAGE_COUNT"]
        assert state.download_requests == []
        assert state.read_since == READ_SINCE
        assert state.read_messages == [USER_THREAD, NEW_REPLY]
        assert OLD_THREAD not in state.read_messages
        methods = [request.get("method") for request in state.requests]
        assert methods == [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call",
            "tools/call",
            "tools/call",
        ]

        fallback_negatives = {
            "wrong_file_id": (pdf, "no receipt for the uploaded PDF"),
            "missing_base64": (pdf, "no uploaded PDF bytes"),
            "invalid_base64": (pdf, "invalid uploaded PDF bytes"),
            "size_mismatch": (pdf, "inconsistent uploaded PDF size"),
            "substituted_same_length": (bytes([pdf[0] ^ 1]) + pdf[1:], "SHA-256 does not match"),
        }
        for mode, (read_pdf, expected_failure) in fallback_negatives.items():
            state.read_mode = mode
            state.read_pdf = read_pdf
            result, fields = run_adapter(server, state, pdf)
            assert result.returncode != 0, f"{mode} hub read must fail closed"
            assert expected_failure in result.stderr, mode
            assert fields is None, f"{mode} must not write a delivery receipt"
            assert state.download_requests == []
            methods = [request.get("method") for request in state.requests]
            assert methods == [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
                "tools/call",
            ], mode
        state.read_mode = "valid"
        state.read_pdf = pdf
        for invalid_thread in ("not-a-slack-timestamp", "1710000000.1234567"):
            result, fields = run_adapter(server, state, pdf, user_thread=invalid_thread)
            assert result.returncode != 0
            assert "USER_THREAD_TS must be a valid Slack timestamp" in result.stderr
            assert fields is None
            assert state.requests == []
            assert state.download_requests == []
        print(
            "release-report-post stub: 6 integrity/endpoint + 6 hub-read "
            "+ 2 timestamp-boundary scenarios passed"
        )
    finally:
        server.shutdown()
        thread.join(timeout=5)
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
