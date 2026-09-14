#!/usr/bin/env python3
"""Post a release report without routing report bytes through an agent context.

The release train passes paths and thread identifiers to this adapter.  The
adapter reads the files locally, speaks the authenticated MechaCassy MCP HTTP
endpoint, and writes the receipt only after both posts have returned usable
receipts.  Credentials are resolved from the environment or the standard
0600 credentials file without sourcing or printing that file.
"""

from __future__ import annotations

import base64
import binascii
from datetime import datetime, timezone
import hashlib
import http.client
import ipaddress
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, NoReturn


DEFAULT_MCP_URL = "https://mecha-cassy.vercel.app/mcp/slack"
DEFAULT_CHANNEL = "cas-internal"
DEFAULT_REPO = "Richards-LLC/cassy"
MCP_PROTOCOL_VERSION = "2025-06-18"
MAX_REMOTE_PDF_BYTES = 32 * 1024 * 1024
MAX_HUB_FILE_BYTES = 4 * 1024 * 1024
SLACK_TIMESTAMP = re.compile(r"^(\d+)(?:\.(\d{1,6}))?$")


class AdapterError(RuntimeError):
    """A safe-to-display adapter failure (never includes secret values)."""


def fail(message: str) -> NoReturn:
    raise AdapterError(message)


def parsed_http_url(url: str, label: str) -> urllib.parse.SplitResult:
    try:
        parsed = urllib.parse.urlsplit(url)
        host = parsed.hostname
        parsed.port
    except ValueError:
        fail(f"{label} is not a valid HTTP(S) URL")
    if parsed.scheme.lower() not in {"http", "https"} or not host:
        fail(f"{label} is not a valid HTTP(S) URL")
    if parsed.username or parsed.password or parsed.fragment:
        fail(f"{label} has unsupported URL credentials or fragment")
    return parsed


def url_origin(parsed: urllib.parse.SplitResult) -> tuple[str, str, int]:
    return (
        parsed.scheme.lower(),
        (parsed.hostname or "").lower(),
        parsed.port or (443 if parsed.scheme.lower() == "https" else 80),
    )


def is_loopback(host: str) -> bool:
    if host.lower() == "localhost":
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def slack_timestamp_since(value: str) -> str:
    match = SLACK_TIMESTAMP.fullmatch(value)
    if not match:
        fail("USER_THREAD_TS must be a valid Slack timestamp (seconds[.fraction])")
    seconds, fraction = match.groups()
    try:
        timestamp = datetime.fromtimestamp(int(seconds), tz=timezone.utc)
    except (OverflowError, OSError, ValueError):
        fail("USER_THREAD_TS must be a valid Slack timestamp (seconds[.fraction])")
    timestamp = timestamp.replace(microsecond=int((fraction or "").ljust(6, "0")))
    return timestamp.isoformat(timespec="microseconds").replace("+00:00", "Z")


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args: Any, **_kwargs: Any) -> None:
        return None


def credential_file() -> Path:
    configured = os.environ.get("CAS_CREDENTIALS_FILE")
    if configured:
        return Path(configured).expanduser()
    xdg = os.environ.get("XDG_CONFIG_HOME")
    base = Path(xdg).expanduser() if xdg else Path.home() / ".config"
    return base / "cas" / "credentials.env"


def parse_credentials(path: Path) -> dict[str, str]:
    """Parse simple exported values, never execute a credentials file."""

    if not path.is_file():
        return {}
    values: dict[str, str] = {}
    assignment = re.compile(r"^\s*(?:export\s+)?([A-Z][A-Z0-9_]*)=(.*)\s*$")
    for line in path.read_text(encoding="utf-8").splitlines():
        match = assignment.match(line)
        if not match:
            continue
        name, raw = match.groups()
        try:
            values[name] = shlex.split(raw, comments=False, posix=True)[0] if raw else ""
        except (ValueError, IndexError):
            continue
    return values


def resolve_secret(name: str, explicit_env: str | None, credentials: dict[str, str]) -> str:
    if explicit_env:
        value = os.environ.get(explicit_env) or credentials.get(explicit_env, "")
        if value:
            return value
        fail(f"credential variable {explicit_env} is unset or empty")
    value = os.environ.get(name, "")
    if value:
        return value
    return credentials.get(name, "")


def resolve_token(credentials: dict[str, str]) -> str:
    explicit = os.environ.get("CAS_RELEASE_TRAIN_MECHA_TOKEN_ENV") or os.environ.get(
        "MECHA_SLACK_TOKEN_ENV"
    )
    token = resolve_secret("MECHA_SLACK_TOKEN", explicit, credentials)
    if token:
        return token

    candidates = sorted(
        {
            name
            for name, value in list(os.environ.items()) + list(credentials.items())
            if name.startswith("MECHA_SLACK_TOKEN_")
            and not name.endswith("_ENV")
            and value
        }
    )
    if len(candidates) == 1:
        return resolve_secret(candidates[0], candidates[0], credentials)
    if len(candidates) > 1:
        names = ", ".join(candidates)
        fail(f"multiple MechaCassy token variables found ({names}); set MECHA_SLACK_TOKEN_ENV")
    fail("no MechaCassy token found; set MECHA_SLACK_TOKEN_ENV or configure credentials.env")


def request_json(
    url: str,
    headers: dict[str, str],
    payload: dict[str, Any],
    timeout: float,
    opener: urllib.request.OpenerDirector | None = None,
) -> tuple[dict[str, Any], dict[str, str]]:
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        request_open = (
            opener if opener is not None else urllib.request.build_opener(NoRedirectHandler())
        ).open
        with request_open(request, timeout=timeout) as response:
            raw = response.read()
            response_headers = {key.lower(): value for key, value in response.headers.items()}
    except urllib.error.HTTPError as exc:
        detail = exc.read(512).decode("utf-8", "replace").replace("\n", " ")
        fail(f"MechaCassy HTTP {exc.code}: {detail[:240]}")
    except urllib.error.URLError as exc:
        fail(f"MechaCassy request failed: {exc.reason}")

    if not raw:
        return {}, response_headers
    content_type = response_headers.get("content-type", "")
    if "text/event-stream" in content_type:
        events = [
            line[5:].strip()
            for line in raw.decode("utf-8").splitlines()
            if line.startswith("data:")
        ]
        if not events:
            return {}, response_headers
        raw = events[-1].encode("utf-8")
    try:
        result = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"MechaCassy returned invalid JSON: {exc}")
    if not isinstance(result, dict):
        fail("MechaCassy returned a non-object JSON response")
    return result, response_headers


class McpClient:
    def __init__(self, url: str, token: str, bypass: str, timeout: float) -> None:
        self.url = url
        self.timeout = timeout
        self.mcp_url = parsed_http_url(url, "MechaCassy MCP URL")
        if self.mcp_url.scheme.lower() == "http" and not is_loopback(self.mcp_url.hostname or ""):
            fail("MechaCassy MCP URL must use HTTPS outside a loopback test endpoint")
        self.opener = urllib.request.build_opener(NoRedirectHandler())
        self.headers = {
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if bypass:
            self.headers["x-vercel-protection-bypass"] = bypass
        self.next_id = 1

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            payload["params"] = params
        result, response_headers = request_json(
            self.url, self.headers, payload, self.timeout, self.opener
        )
        session_id = response_headers.get("mcp-session-id")
        if session_id:
            self.headers["Mcp-Session-Id"] = session_id
        if "error" in result:
            error = result["error"]
            if isinstance(error, dict):
                fail(
                    f"MechaCassy MCP error {error.get('code', 'unknown')}: "
                    f"{error.get('message', 'unknown')}"
                )
            fail("MechaCassy MCP returned an error")
        response = result.get("result")
        if not isinstance(response, dict):
            fail(f"MechaCassy MCP response for {method} had no result")
        return response

    def notify_initialized(self) -> None:
        payload = {"jsonrpc": "2.0", "method": "notifications/initialized"}
        request_json(self.url, self.headers, payload, self.timeout, self.opener)

    def download(self, url: str) -> bytes:
        """Download a verified endpoint without crossing its credential boundary."""

        current_url = url
        opener = self.opener
        for redirect_count in range(4):
            current = parsed_http_url(current_url, "PDF download URL")
            current_origin = url_origin(current)
            mcp_origin = url_origin(self.mcp_url)
            same_origin = current_origin == mcp_origin
            if current.scheme.lower() == "http" and not (
                same_origin
                and self.mcp_url.scheme.lower() == "http"
                and is_loopback(current.hostname or "")
            ):
                fail("PDF download URL must use HTTPS outside a same-origin loopback test endpoint")
            authenticated = same_origin
            headers = {
                "Accept": "application/pdf, application/octet-stream",
            }
            if authenticated:
                headers["Authorization"] = self.headers["Authorization"]
                if "x-vercel-protection-bypass" in self.headers:
                    headers["x-vercel-protection-bypass"] = self.headers[
                        "x-vercel-protection-bypass"
                    ]
            request = urllib.request.Request(current_url, headers=headers, method="GET")
            try:
                response = opener.open(request, timeout=self.timeout)
            except urllib.error.HTTPError as exc:
                if 300 <= exc.code < 400:
                    headers = getattr(exc, "headers", None) or {}
                    exc.close()
                    current_url = self._redirect_url(
                        current_url,
                        current,
                        current_origin,
                        authenticated,
                        headers.get("Location", ""),
                    )
                    continue
                fail(f"MechaCassy PDF download returned HTTP {exc.code}")
            except urllib.error.URLError as exc:
                fail(f"MechaCassy PDF download failed: {exc.reason}")
            except (http.client.HTTPException, OSError) as exc:
                fail(f"MechaCassy PDF download failed ({type(exc).__name__})")

            status = getattr(response, "status", None)
            if status is None:
                status = response.getcode()
            if status is not None and 300 <= status < 400:
                location = response.headers.get("Location", "")
                response.close()
                current_url = self._redirect_url(
                    current_url,
                    current,
                    current_origin,
                    authenticated,
                    location,
                )
                continue
            if status is not None and not 200 <= status < 300:
                response.close()
                fail(f"MechaCassy PDF download returned HTTP {status}")
            try:
                raw = response.read(MAX_REMOTE_PDF_BYTES + 1)
            except (http.client.HTTPException, OSError) as exc:
                fail(f"MechaCassy PDF download failed ({type(exc).__name__})")
            finally:
                response.close()
            if len(raw) > MAX_REMOTE_PDF_BYTES:
                fail("MechaCassy PDF download exceeded the safety limit")
            if not raw:
                fail("MechaCassy PDF download returned no bytes")
            return raw
        fail("PDF download followed too many same-origin redirects")

    def _redirect_url(
        self,
        current_url: str,
        current: urllib.parse.SplitResult,
        current_origin: tuple[str, str, int],
        authenticated: bool,
        location: str,
    ) -> str:
        if not location:
            fail("PDF download redirect had no Location")
        next_url = urllib.parse.urljoin(current_url, location)
        next_parsed = parsed_http_url(next_url, "PDF download redirect")
        next_origin = url_origin(next_parsed)
        if authenticated and next_origin != url_origin(self.mcp_url):
            fail("authenticated PDF download redirect left the MCP origin")
        if next_origin != current_origin:
            fail("PDF download redirect changed its origin")
        if next_parsed.scheme.lower() != current.scheme.lower():
            fail("PDF download redirect changed its scheme")
        return next_url

    def tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        result = self.call("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            fail(f"MechaCassy tool {name} returned an error")
        structured = result.get("structuredContent")
        if isinstance(structured, dict):
            return structured
        for item in result.get("content", []):
            if isinstance(item, dict) and item.get("type") == "text":
                try:
                    decoded = json.loads(item.get("text", ""))
                except json.JSONDecodeError:
                    continue
                if isinstance(decoded, dict):
                    return decoded
        fail(f"MechaCassy tool {name} returned no JSON envelope")

    def read_file(self, channel: str, file_id: str, since: str) -> bytes:
        """Read an uploaded file through the hub's authenticated file packer."""

        result = self.tool(
            "mecha_read",
            {
                "channel": channel,
                "since": since,
                "include_files": True,
                "include_threads": True,
                "max_messages": 500,
                "max_files": 50,
                "max_file_bytes": MAX_HUB_FILE_BYTES,
                "max_bytes": 8 * 1024 * 1024,
            },
        )
        files = result.get("files")
        if not isinstance(files, list):
            fail("MechaCassy file read returned no file receipts")
        for file in files:
            if not isinstance(file, dict) or file.get("file_id") != file_id:
                continue
            content = file.get("content_base64")
            if not isinstance(content, str) or not content:
                fail("MechaCassy file read returned no uploaded PDF bytes")
            try:
                raw = base64.b64decode(content, validate=True)
            except (ValueError, binascii.Error):
                fail("MechaCassy file read returned invalid uploaded PDF bytes")
            size = file.get("size_bytes")
            if not isinstance(size, int) or size != len(raw):
                fail("MechaCassy file read returned inconsistent uploaded PDF size")
            return raw
        fail("MechaCassy file read returned no receipt for the uploaded PDF")


def message_receipt(envelope: dict[str, Any], label: str) -> tuple[str, str]:
    message = envelope.get("message")
    if not isinstance(message, dict):
        fail(f"{label} post returned no message receipt")
    message_id = message.get("message_id") or message.get("id")
    permalink = message.get("permalink")
    if not isinstance(message_id, str) or not message_id.strip():
        fail(f"{label} post returned no message id")
    if not isinstance(permalink, str) or not permalink.startswith("https://"):
        fail(f"{label} post returned no HTTPS permalink")
    return message_id, permalink


def file_receipt(envelope: dict[str, Any]) -> tuple[str, str, str | None]:
    message_id, message_permalink = message_receipt(envelope, "PDF")
    file_block = envelope.get("file")
    if not isinstance(file_block, dict):
        download_url = envelope.get("download_url")
        if download_url is None:
            fail("PDF post returned no verified PDF download URL; message permalink is not a PDF endpoint")
        return message_id, message_permalink, checked_download_url(download_url)
    file_id = file_block.get("file_id") or file_block.get("id") or message_id
    file_permalink = file_block.get("permalink") or file_block.get("url") or message_permalink
    if not isinstance(file_id, str) or not file_id.strip():
        fail("PDF post returned no file id")
    if not isinstance(file_permalink, str) or not file_permalink.startswith("https://"):
        fail("PDF post returned no HTTPS file permalink")
    download_url = file_block.get("download_url")
    return file_id, file_permalink, checked_download_url(download_url) if download_url is not None else None


def checked_download_url(value: Any) -> str:
    if (
        not isinstance(value, str)
        or any(character.isspace() for character in value)
    ):
        fail("PDF post returned no usable HTTP(S) download URL")
    parsed_http_url(value, "PDF download URL")
    return value


def page_count(pdf_path: Path, pdf_bytes: bytes) -> int:
    if not pdf_bytes.startswith(b"%PDF-"):
        fail(f"PDF does not start with a PDF header: {pdf_path}")
    try:
        result = subprocess.run(
            ["pdfinfo", str(pdf_path)],
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (FileNotFoundError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        result = None
    if result:
        for line in result.stdout.splitlines():
            if line.startswith("Pages:"):
                value = line.split(":", 1)[1].strip()
                if value.isdigit() and int(value) > 0:
                    return int(value)
    matches = re.findall(rb"/Type\s*/Page(?:[^A-Za-z0-9]|$)", pdf_bytes)
    if matches:
        return len(matches)
    fail(f"could not determine PDF page count: {pdf_path}")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def verify_remote_pdf(
    client: McpClient,
    channel: str,
    file_id: str,
    download_url: str | None,
    since: str,
    local_bytes: bytes,
    local_sha: str,
    local_pages: int,
) -> tuple[str, int, int]:
    """Prove the transport's stored PDF is the exact local, decodable PDF."""

    remote_bytes = (
        client.download(download_url)
        if download_url
        else client.read_file(channel, file_id, since)
    )
    remote_size = len(remote_bytes)
    if remote_size != len(local_bytes):
        fail(
            "uploaded PDF byte count mismatch "
            f"(local={len(local_bytes)}, remote={remote_size})"
        )
    remote_sha = sha256(remote_bytes)
    if remote_sha != local_sha:
        fail("uploaded PDF SHA-256 does not match the local PDF")

    with tempfile.NamedTemporaryFile(prefix="cassy-release-report-", suffix=".pdf") as remote_file:
        remote_file.write(remote_bytes)
        remote_file.flush()
        remote_pages = page_count(Path(remote_file.name), remote_bytes)
    if remote_pages != local_pages:
        fail(
            "uploaded PDF page count mismatch "
            f"(local={local_pages}, remote={remote_pages})"
        )
    return remote_sha, remote_size, remote_pages


def html_url(tag: str) -> str:
    configured = os.environ.get("CAS_RELEASE_TRAIN_REPORT_HTML_URL")
    if configured:
        return configured
    repo = os.environ.get("CAS_RELEASE_TRAIN_REPO", DEFAULT_REPO).strip().strip("/")
    ref = os.environ.get("CAS_RELEASE_TRAIN_REPORT_HTML_REF", "main").strip()
    return f"https://github.com/{repo}/blob/{ref}/docs/release-reports/{tag}.html"


def write_receipt(path: Path, fields: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            for key, value in fields.items():
                stream.write(f"{key}={value}\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def main(argv: list[str]) -> int:
    if len(argv) != 6:
        print(
            "usage: release-report-post.py TAG PDF_PATH HTML_PATH USER_THREAD_TS DEV_THREAD_TS",
            file=sys.stderr,
        )
        return 2
    tag, pdf_arg, html_arg, user_thread, dev_thread = argv[1:]
    if not tag.startswith("v") or not user_thread or not dev_thread:
        print(
            "error: TAG must start with v and both thread timestamps are required",
            file=sys.stderr,
        )
        return 2
    receipt_arg = os.environ.get("CAS_RELEASE_TRAIN_REPORT_RECEIPT")
    if not receipt_arg:
        print("error: CAS_RELEASE_TRAIN_REPORT_RECEIPT is required", file=sys.stderr)
        return 2
    pdf_path = Path(pdf_arg).expanduser().resolve()
    html_path = Path(html_arg).expanduser().resolve()
    receipt_path = Path(receipt_arg).expanduser().resolve()
    if not pdf_path.is_file() or not html_path.is_file():
        print("error: PDF_PATH and HTML_PATH must name existing files", file=sys.stderr)
        return 2
    worktree = os.environ.get("CAS_RELEASE_TRAIN_REPORT_WORKTREE")
    if worktree:
        root = Path(worktree).expanduser().resolve()
        if root not in pdf_path.parents or root not in html_path.parents:
            print(
                "error: report paths must remain inside CAS_RELEASE_TRAIN_REPORT_WORKTREE",
                file=sys.stderr,
            )
            return 2

    try:
        read_since = slack_timestamp_since(user_thread)
        pdf_bytes = pdf_path.read_bytes()
        html_bytes = html_path.read_bytes()
        pdf_sha = sha256(pdf_bytes)
        html_sha = sha256(html_bytes)
        pages = page_count(pdf_path, pdf_bytes)
        credentials = parse_credentials(credential_file())
        token = resolve_token(credentials)
        bypass = resolve_secret("MECHA_VERCEL_BYPASS", None, credentials)
        url = os.environ.get("CAS_RELEASE_TRAIN_REPORT_MCP_URL", DEFAULT_MCP_URL)
        channel = os.environ.get("CAS_RELEASE_TRAIN_REPORT_CHANNEL", DEFAULT_CHANNEL)
        timeout = float(os.environ.get("CAS_RELEASE_TRAIN_REPORT_TIMEOUT_SECS", "30"))
        client = McpClient(url, token, bypass, timeout)
        client.call(
            "initialize",
            {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "cassy-release-report", "version": "1"},
            },
        )
        client.notify_initialized()
        tools = client.call("tools/list")
        tool_names = {
            tool.get("name")
            for tool in tools.get("tools", [])
            if isinstance(tool, dict) and isinstance(tool.get("name"), str)
        }
        if "mecha_post" not in tool_names:
            fail("authenticated MechaCassy tools/list does not expose mecha_post")

        pdf_envelope = client.tool(
            "mecha_post",
            {
                "channel": channel,
                "kind": "file",
                "reply_to": user_thread,
                "file": {
                    "filename": pdf_path.name,
                    "title": f"Cassy {tag} release report",
                    "content": base64.b64encode(pdf_bytes).decode("ascii"),
                    "content_encoding": "base64",
                },
            },
        )
        pdf_file_id, pdf_permalink, pdf_download_url = file_receipt(pdf_envelope)
        remote_pdf_sha, remote_pdf_size, remote_pdf_pages = verify_remote_pdf(
            client, channel, pdf_file_id, pdf_download_url, read_since, pdf_bytes, pdf_sha, pages
        )
        html_envelope = client.tool(
            "mecha_post",
            {
                "channel": channel,
                "kind": "message",
                "reply_to": dev_thread,
                "text": f"Release report HTML: <{html_url(tag)}|open the standalone report>",
            },
        )
        html_file_id, _html_permalink = message_receipt(html_envelope, "HTML")
        write_receipt(
            receipt_path,
            {
                "TAG": tag,
                "PDF_PATH": str(pdf_path),
                "HTML_PATH": str(html_path),
                "PDF_SHA256": pdf_sha,
                "PDF_SIZE_BYTES": str(len(pdf_bytes)),
                "PDF_REMOTE_SHA256": remote_pdf_sha,
                "PDF_REMOTE_SIZE_BYTES": str(remote_pdf_size),
                "PDF_REMOTE_PAGE_COUNT": str(remote_pdf_pages),
                "HTML_SHA256": html_sha,
                "PAGE_COUNT": str(pages),
                "PDF_FILE_PERMALINK": pdf_permalink,
                "PDF_FILE_ID": pdf_file_id,
                "HTML_FILE_ID": html_file_id,
                "USER_THREAD_TS": user_thread,
                "DEV_THREAD_TS": dev_thread,
            },
        )
        print(f"release report posted: {tag} PDF_FILE_ID={pdf_file_id} HTML_FILE_ID={html_file_id}")
        return 0
    except (AdapterError, OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
