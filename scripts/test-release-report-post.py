#!/usr/bin/env python3
"""Stub-endpoint proof for release-report-post.py."""

from __future__ import annotations

import base64
import json
import os
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ADAPTER = ROOT / "scripts" / "release-report-post.py"
PDF = ROOT / "docs" / "release-reports" / "v3.19.0.pdf"


class StubState:
    def __init__(self, pdf: bytes) -> None:
        self.pdf = pdf
        self.requests: list[dict] = []


def envelope(result: dict) -> bytes:
    return json.dumps({"jsonrpc": "2.0", "id": result.pop("_id", 1), "result": result}).encode()


def handler_for(state: StubState):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format: str, *_args: object) -> None:
            return

        def do_POST(self) -> None:  # noqa: N802
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
                arguments = request["params"]["arguments"]
                assert request["params"]["name"] == "mecha_post"
                if arguments["kind"] == "file":
                    assert arguments["reply_to"] == "user-thread"
                    assert arguments["file"]["content_encoding"] == "base64"
                    assert base64.b64decode(arguments["file"]["content"]) == state.pdf
                    result = {
                        "_id": request_id,
                        "content": [
                            {
                                "type": "text",
                                "text": json.dumps(
                                    {
                                        "ok": True,
                                        "schema_version": 1,
                                        "kind": "file",
                                        "message": {
                                            "message_id": "file-message",
                                            "thread_id": "user-thread",
                                            "permalink": "https://example.test/files/report.pdf",
                                        },
                                        "file": {
                                            "file_id": "F-report",
                                            "permalink": "https://example.test/files/report.pdf",
                                        },
                                    }
                                ),
                            }
                        ],
                    }
                else:
                    assert arguments["kind"] == "message"
                    assert arguments["reply_to"] == "dev-thread"
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
                                            "thread_id": "dev-thread",
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


def main() -> int:
    pdf = PDF.read_bytes()
    state = StubState(pdf)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(state))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
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
                    "CAS_RELEASE_TRAIN_REPORT_USER_THREAD_TS": "user-thread",
                    "CAS_RELEASE_TRAIN_REPORT_DEV_THREAD_TS": "dev-thread",
                    "CAS_CREDENTIALS_FILE": str(credentials),
                    "CAS_RELEASE_TRAIN_MECHA_TOKEN_ENV": "MECHA_SLACK_TOKEN_TEST",
                    "CAS_RELEASE_TRAIN_REPORT_REPO": "Richards-LLC/cassy",
                }
            )
            environment.pop("MECHA_SLACK_TOKEN_TEST", None)
            environment.pop("MECHA_VERCEL_BYPASS", None)
            result = subprocess.run(
                [sys.executable, str(ADAPTER), "v9.99.0", str(PDF), str(html), "user-thread", "dev-thread"],
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )
            if result.returncode != 0:
                raise AssertionError(f"adapter failed: stdout={result.stdout!r} stderr={result.stderr!r}")
            fields = dict(line.split("=", 1) for line in receipt.read_text(encoding="utf-8").splitlines())
            assert fields["TAG"] == "v9.99.0"
            assert fields["PDF_FILE_ID"] == "F-report"
            assert fields["HTML_FILE_ID"] == "html-message"
            assert fields["USER_THREAD_TS"] == "user-thread"
            assert fields["DEV_THREAD_TS"] == "dev-thread"
            assert len(fields["PDF_SHA256"]) == 64
            assert int(fields["PAGE_COUNT"]) > 0
            methods = [request.get("method") for request in state.requests]
            assert methods == ["initialize", "notifications/initialized", "tools/list", "tools/call", "tools/call"]
            print("release-report-post stub: 1 scenario passed")
    finally:
        server.shutdown()
        thread.join(timeout=5)
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
