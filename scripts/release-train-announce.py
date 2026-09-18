#!/usr/bin/env python3
"""Validate and post the four release-draft bodies through MechaCassy."""

from __future__ import annotations

from datetime import datetime, timezone
import importlib.util
import os
from pathlib import Path
import re
import sys
import time
from typing import Any


ROOT = Path(__file__).resolve().parent
REPORT_ADAPTER = ROOT / "release-report-post.py"
BODY_NAMES = ("user-top-level", "user-reply", "dev-top-level", "dev-reply")
USER_FORBIDDEN = (
    "watermark",
    "metadata",
    "ingest",
    "upsert",
    "envelope",
    "scope stamp",
    "canonical id",
    "team pull",
    "purge-foreign",
    "cloud identity",
    "registration",
    "harness",
    "agent",
    "factory",
    "worker",
    "supervisor",
    "epic",
    "lane",
)


def fail(message: str) -> None:
    raise ValueError(message)


def body_lines(body: str) -> list[str]:
    return body.strip("\n").splitlines()


def lint_body(index: int, body: str) -> None:
    lines = body_lines(body)
    if not lines:
        fail(f"body {BODY_NAMES[index]} is empty")
    if "**" in body:
        fail(f"body {BODY_NAMES[index]} contains forbidden ** markdown")
    for line_number, line in enumerate(lines, start=1):
        if line.startswith("#"):
            fail(f"body {BODY_NAMES[index]} line {line_number} starts with #")
        if re.match(r"^-\s", line):
            fail(f"body {BODY_NAMES[index]} line {line_number} uses a hyphen bullet")
    bullets = [number for number, line in enumerate(lines) if line.startswith("• ")]
    if index in (0, 2):
        if len(lines) != 2 or not lines[0].startswith("*Live on "):
            fail(f"body {BODY_NAMES[index]} top-level must contain exactly two lines")
        if "Was:" not in lines[1] or "→ Now:" not in lines[1]:
            fail(f"body {BODY_NAMES[index]} top-level must use Was → Now")
        if len(re.findall(r"[A-Za-z0-9][A-Za-z0-9’'-]*", lines[1])) > 25:
            fail(f"body {BODY_NAMES[index]} top-level punch exceeds 25 words")
        if index == 0:
            lowered = body.lower()
            for forbidden in USER_FORBIDDEN:
                if forbidden in lowered:
                    fail(f"body {BODY_NAMES[index]} contains forbidden user wording: {forbidden}")
        return
    if not bullets:
        fail(f"body {BODY_NAMES[index]} has no bullet")
    for position, line_number in enumerate(bullets):
        if not re.match(r"^• \*[^*\n]+\* — ", lines[line_number]):
            fail(f"body {BODY_NAMES[index]} bullet lacks a bold label")
        if position and (line_number == 0 or lines[line_number - 1].strip()):
            fail(f"body {BODY_NAMES[index]} bullets need blank lines between items")
        next_bullet = bullets[position + 1] if position + 1 < len(bullets) else len(lines)
        nonempty = [line for line in lines[line_number:next_bullet] if line.strip()]
        if len(nonempty) > 2:
            fail(f"body {BODY_NAMES[index]} bullet spans more than two lines")


def extract_bodies(draft: Path) -> list[str]:
    text = draft.read_text(encoding="utf-8")
    bodies = re.findall(r"\x60\x60\x60(?:text)?\r?\n(.*?)\r?\n\x60\x60\x60", text, flags=re.DOTALL)
    if len(bodies) != 4:
        fail(f"draft must contain exactly four fenced bodies; found {len(bodies)}")
    for index, body in enumerate(bodies):
        lint_body(index, body)
    return bodies


def validate(draft_arg: str, body_dir_arg: str) -> None:
    draft = Path(draft_arg).expanduser().resolve()
    body_dir = Path(body_dir_arg).expanduser().resolve()
    if not draft.is_file():
        fail(f"draft does not exist: {draft}")
    bodies = extract_bodies(draft)
    body_dir.mkdir(parents=True, exist_ok=True)
    for name, body in zip(BODY_NAMES, bodies):
        (body_dir / f"{name}.txt").write_text(body + "\n", encoding="utf-8")
    print(f"announce lint PASS · bodies=4 · draft={draft}")


def load_report_adapter() -> Any:
    spec = importlib.util.spec_from_file_location("release_report_post", REPORT_ADAPTER)
    if spec is None or spec.loader is None:
        fail(f"cannot load authenticated adapter: {REPORT_ADAPTER}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def proxy_token_env() -> str | None:
    configured = os.environ.get("MECHA_SLACK_TOKEN_ENV") or os.environ.get(
        "CAS_RELEASE_TRAIN_MECHA_TOKEN_ENV"
    )
    if configured:
        return configured
    proxy = os.environ.get("CAS_RELEASE_TRAIN_PROXY_TOML")
    if not proxy:
        return None
    try:
        text = Path(proxy).read_text(encoding="utf-8")
    except OSError:
        return None
    match = re.search(r'^\s*auth\s*=\s*"env:([A-Z][A-Z0-9_]*)"\s*$', text, flags=re.MULTILINE)
    return match.group(1) if match else None


def receipt(path: Path, values: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.partial")
    temporary.write_text(
        "".join(f"{key}={value}\n" for key, value in values.items()), encoding="utf-8"
    )
    temporary.replace(path)


def post(version: str, draft_arg: str, receipt_arg: str, body_dir_arg: str) -> None:
    adapter = load_report_adapter()
    draft = Path(draft_arg).expanduser().resolve()
    receipt_path = Path(receipt_arg).expanduser().resolve()
    body_dir = Path(body_dir_arg).expanduser().resolve()
    bodies = extract_bodies(draft)
    for name, body in zip(BODY_NAMES, bodies):
        expected = body_dir / f"{name}.txt"
        if not expected.is_file() or expected.read_text(encoding="utf-8").strip("\n") != body:
            fail(f"validated body is missing or changed: {expected}")
    credentials = adapter.parse_credentials(adapter.credential_file())
    token_env = proxy_token_env()
    if token_env and not os.environ.get("MECHA_SLACK_TOKEN_ENV"):
        os.environ["MECHA_SLACK_TOKEN_ENV"] = token_env
    token = adapter.resolve_token(credentials)
    bypass = adapter.resolve_secret("MECHA_VERCEL_BYPASS", None, credentials)
    url = os.environ.get("CAS_RELEASE_TRAIN_ANNOUNCE_MCP_URL", adapter.DEFAULT_MCP_URL)
    channel = os.environ.get("CAS_RELEASE_TRAIN_ANNOUNCE_CHANNEL", adapter.DEFAULT_CHANNEL)
    timeout = float(os.environ.get("CAS_RELEASE_TRAIN_ANNOUNCE_TIMEOUT_SECS", "30"))
    client = adapter.McpClient(url, token, bypass, timeout)
    client.call(
        "initialize",
        {
            "protocolVersion": adapter.MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "cassy-release-train", "version": "1"},
        },
    )
    client.notify_initialized()
    tools = client.call("tools/list")
    names = {
        item.get("name")
        for item in tools.get("tools", [])
        if isinstance(item, dict) and isinstance(item.get("name"), str)
    }
    if names != {"mecha_read", "mecha_post"}:
        fail("authenticated MechaCassy tools/list must expose exactly mecha_read and mecha_post")
    since = os.environ.get(
        "CAS_RELEASE_TRAIN_ANNOUNCE_READ_SINCE",
        datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z"),
    )
    client.tool(
        "mecha_read",
        {
            "channel": channel,
            "since": since,
            "max_messages": 500,
            "include_threads": True,
            "include_files": False,
            "max_files": 50,
            "max_file_bytes": 4 * 1024 * 1024,
            "max_bytes": 8 * 1024 * 1024,
        },
    )
    paths = [body_dir / f"{name}.txt" for name in BODY_NAMES]
    texts = [path.read_text(encoding="utf-8").rstrip("\n") for path in paths]
    posted_at = datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    receipt_values: dict[str, str] = {"POSTED_AT": posted_at, "CHANNEL": channel}
    parent: str | None = None
    dev_parent: str | None = None
    for index, text in enumerate(texts):
        arguments: dict[str, Any] = {"channel": channel, "kind": "message", "text": text}
        if index == 1:
            arguments["reply_to"] = parent
        elif index == 3:
            arguments["reply_to"] = dev_parent
        envelope = client.tool("mecha_post", arguments)
        message_id, permalink = adapter.message_receipt(envelope, BODY_NAMES[index])
        if index == 0:
            channel_block = envelope.get("channel")
            if isinstance(channel_block, dict) and isinstance(channel_block.get("id"), str):
                receipt_values["CHANNEL_ID"] = channel_block["id"]
        prefix = ("USER_TOP_LEVEL" if index == 0 else
                  "USER_REPLY" if index == 1 else
                  "DEV_TOP_LEVEL" if index == 2 else "DEV_REPLY")
        receipt_values[f"{prefix}_ID"] = message_id
        receipt_values[f"{prefix}_PERMALINK"] = permalink
        receipt(receipt_path, receipt_values)
        if index == 0:
            parent = message_id
        elif index == 2:
            dev_parent = message_id
        if index < len(texts) - 1:
            time.sleep(1)
    receipt(receipt_path, receipt_values)
    print(f"announce posted · version=v{version} · receipt={receipt_path}")


def main(argv: list[str]) -> int:
    try:
        if len(argv) == 4 and argv[1] == "--validate":
            validate(argv[2], argv[3])
            return 0
        if len(argv) == 6 and argv[1] == "--post":
            post(argv[2], argv[3], argv[4], argv[5])
            return 0
        print(
            "usage: release-train-announce.py --validate DRAFT BODY_DIR | "
            "--post VERSION DRAFT RECEIPT BODY_DIR",
            file=sys.stderr,
        )
        return 2
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
