#!/usr/bin/env python3
"""Validate and post the four release-draft bodies through Violet."""

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
    "daemon",
    "epic",
    "lane",
)
TOP_LEVEL_LABEL = re.compile(
    r"^\*(?:Live on production|Staging|Source on main) — "
    r"(?:User|Dev) — Cassy(?: v[0-9]+\.[0-9]+\.[0-9]+)?\*$"
)


def fail(message: str) -> None:
    raise ValueError(message)


def body_lines(body: str) -> list[str]:
    return body.strip("\n").splitlines()


# post-publication fills these from the published assets; they are the only
# tokens a draft may carry before publication (preflight).
DIGEST_TOKENS = frozenset({"{{LINUX_SHA256}}", "{{MACOS_SHA256}}"})


def lint_body(index: int, body: str, allow_digest_tokens: bool = False) -> None:
    lines = body_lines(body)
    if not lines:
        fail(f"body {BODY_NAMES[index]} is empty")
    token = next(
        (match for match in re.finditer(r"\{\{.*?\}\}", body.strip("\n"), flags=re.DOTALL)
         if not (allow_digest_tokens and match.group() in DIGEST_TOKENS)),
        None,
    )
    if token:
        line_number = body.strip("\n")[:token.start()].count("\n") + 1
        fail(f"body {BODY_NAMES[index]} line {line_number} contains unresolved token {token.group()}")
    if "**" in body:
        fail(f"body {BODY_NAMES[index]} contains forbidden ** markdown")
    for line_number, line in enumerate(lines, start=1):
        if line.startswith("#"):
            fail(f"body {BODY_NAMES[index]} line {line_number} starts with #")
        if re.match(r"^-\s", line):
            fail(f"body {BODY_NAMES[index]} line {line_number} uses a hyphen bullet")
        if index == 0:
            lowered = line.lower()
            for forbidden in USER_FORBIDDEN:
                if forbidden in lowered:
                    fail(
                        f"body {BODY_NAMES[index]} line {line_number} contains forbidden "
                        f"user wording: {forbidden}: {line}"
                    )
    bullets = [number for number, line in enumerate(lines) if line.startswith("• ")]
    if index in (0, 2):
        if len(lines) != 2 or not TOP_LEVEL_LABEL.fullmatch(lines[0]):
            fail(f"body {BODY_NAMES[index]} top-level must contain exactly two lines")
        if "Was:" not in lines[1] or "→ Now:" not in lines[1]:
            fail(f"body {BODY_NAMES[index]} top-level must use Was → Now")
        if len(re.findall(r"[A-Za-z0-9][A-Za-z0-9’'-]*", lines[1])) > 25:
            fail(f"body {BODY_NAMES[index]} top-level punch exceeds 25 words")
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


def extract_bodies(draft: Path, allow_digest_tokens: bool = False) -> list[str]:
    text = draft.read_text(encoding="utf-8")
    bodies = re.findall(r"\x60\x60\x60(?:text)?\r?\n(.*?)\r?\n\x60\x60\x60", text, flags=re.DOTALL)
    if len(bodies) != 4:
        fail(f"draft must contain exactly four fenced bodies; found {len(bodies)}")
    for index, body in enumerate(bodies):
        lint_body(index, body, allow_digest_tokens)
    return bodies


def validate(draft_arg: str, body_dir_arg: str, pre_publication: bool = False) -> None:
    draft = Path(draft_arg).expanduser().resolve()
    body_dir = Path(body_dir_arg).expanduser().resolve()
    if not draft.is_file():
        fail(f"draft does not exist: {draft}")
    bodies = extract_bodies(draft, allow_digest_tokens=pre_publication)
    body_dir.mkdir(parents=True, exist_ok=True)
    for name, body in zip(BODY_NAMES, bodies):
        (body_dir / f"{name}.txt").write_text(body + "\n", encoding="utf-8")
    print(f"announce lint PASS · bodies=4 · draft={draft}")


def record_latency(tag: str, receipt_arg: str, draft_arg: str) -> None:
    """Put measured release effort in both replies and timing in the Dev reply."""
    values = {}
    for line in Path(receipt_arg).read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or key in values:
            fail("missing or incoherent latency measurement")
        values[key] = value
    seconds = values.get("PUBLISH_LATENCY_SECONDS", "")
    budget = values.get("BUDGET_SECONDS", "")
    within = values.get("WITHIN_BUDGET", "")
    interventions = values.get("INTERVENTIONS", "")
    if (values.get("TAG") != tag or not seconds.isascii() or not seconds.isdecimal()
            or not budget.isascii() or not budget.isdecimal() or within not in {"true", "false"}
            or not interventions.isascii() or not interventions.isdecimal()):
        fail("missing or incoherent latency measurement")
    try:
        start = datetime.fromisoformat(values["TAG_PUSHED_AT"].replace("Z", "+00:00"))
        end = datetime.fromisoformat(values["PUBLISHED_AT"].replace("Z", "+00:00"))
    except (KeyError, ValueError):
        fail("missing or incoherent latency measurement")
    if (start.tzinfo is None or end.tzinfo is None or (end - start).total_seconds() != int(seconds)
            or (within == "true") != (int(seconds) <= int(budget))):
        fail("missing or incoherent latency measurement")
    draft = Path(draft_arg)
    if not draft.is_file():
        return  # Standalone receipt collection may have no announcement draft.
    bodies = extract_bodies(draft)
    state = "within budget" if within == "true" else "over budget"
    line = (f"• *Publication timing* — Tag to published: {seconds}s; {state} ({budget}s); "
            f"WITHIN_BUDGET={within}.")
    effort = f"• *Release effort* — {int(interventions)} manual interventions were needed to publish this release."
    for index in (1, 3):
        body = re.sub(r"(?m)^• \*(?:Release effort|Publication timing)\* — .*\n?", "", bodies[index]).rstrip()
        bodies[index] = body + "\n\n" + effort
    bodies[3] += "\n\n" + line + f" INTERVENTIONS={int(interventions)}."
    for index in (1, 3):
        lint_body(index, bodies[index])
    source = draft.read_text(encoding="utf-8")
    fences = list(re.finditer(r"\x60\x60\x60(?:text)?\r?\n(.*?)\r?\n\x60\x60\x60", source, re.DOTALL))
    updated = source
    for index in (3, 1):
        match = fences[index]
        updated = updated[:match.start(1)] + bodies[index] + updated[match.end(1):]
    if updated != source:
        temporary = draft.with_name(f".{draft.name}.latency")
        temporary.write_text(updated, encoding="utf-8")
        temporary.chmod(draft.stat().st_mode & 0o777)
        temporary.replace(draft)


def load_report_adapter() -> Any:
    spec = importlib.util.spec_from_file_location("release_report_post", REPORT_ADAPTER)
    if spec is None or spec.loader is None:
        fail(f"cannot load authenticated adapter: {REPORT_ADAPTER}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def proxy_token_env() -> str | None:
    compatibility = load_report_adapter().COMPATIBILITY
    configured = (os.environ.get("VIOLET_SLACK_TOKEN_ENV")
                  or os.environ.get("CAS_RELEASE_TRAIN_VIOLET_TOKEN_ENV")
                  or os.environ.get(load_report_adapter().LEGACY_TOKEN_SELECTOR_ENV)
                  or os.environ.get(compatibility["legacy_train_selector_env"]))
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


def announce_token_env(credentials: dict[str, str]) -> str | None:
    """The token variable to pin for the adapter, or None to let it choose.

    cas-fed5: the project proxy.toml names the token variable of the host it
    was written on. A different host (a Mac cutting a release) may not set
    that variable; pinning it anyway failed the announce with "unset or
    empty" although this machine's registered Violet token was present.
    A proxy-derived name is pinned only when it resolves here; otherwise the
    adapter picks this machine's registered or only token. An operator's
    explicit token selector is never overridden.
    """

    if os.environ.get("VIOLET_SLACK_TOKEN_ENV") or os.environ.get(load_report_adapter().LEGACY_TOKEN_SELECTOR_ENV):
        return None
    token_env = proxy_token_env()
    if not token_env:
        return None
    adapter = load_report_adapter()
    if any(os.environ.get(name) or credentials.get(name) for name in adapter.credential_names(token_env)):
        return token_env
    return None


def resolve_announce_token(adapter: Any, credentials: dict[str, str]) -> str:
    """Share local credential selection between preflight and posting."""
    token_env = announce_token_env(credentials)
    if token_env:
        os.environ["VIOLET_SLACK_TOKEN_ENV"] = token_env
    return adapter.resolve_token(credentials)


def check_token() -> None:
    # Resolve locally, without creating an MCP client or displaying the secret.
    adapter = load_report_adapter()
    resolve_announce_token(adapter, adapter.parse_credentials(adapter.credential_file()))
    print("announce token resolution PASS")


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
    token = resolve_announce_token(adapter, credentials)
    bypass = adapter.resolve_secret("VIOLET_VERCEL_BYPASS", None, credentials)
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
    if not {"violet_read", "violet_post"}.issubset(names):
        fail("authenticated Violet tools/list must expose violet_read and violet_post")
    since = os.environ.get(
        "CAS_RELEASE_TRAIN_ANNOUNCE_READ_SINCE",
        datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z"),
    )
    client.tool(
        "violet_read",
        {
            "channel": channel,
            "since": since,
            "max_messages": 500,
            "include_threads": True,
            "include_files": False,
            "include_channels": False,
            "max_files": 50,
            "max_file_bytes": 4 * 1024 * 1024,
            "max_bytes": 8 * 1024 * 1024,
        },
    )
    paths = [body_dir / f"{name}.txt" for name in BODY_NAMES]
    texts = [path.read_text(encoding="utf-8").rstrip("\n") for path in paths]
    # The membership read may take time. Check every captured body again before
    # the first write, then post these same strings without reopening the files.
    for index, text in enumerate(texts):
        lint_body(index, text)
        if text != bodies[index]:
            fail(f"validated body is changed: {paths[index]}")
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
        envelope = client.tool("violet_post", arguments)
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
        if len(argv) == 2 and argv[1] == "--check-token":
            check_token()
            return 0
        if len(argv) == 5 and argv[1] == "--record-latency":
            record_latency(argv[2], argv[3], argv[4])
            return 0
        if len(argv) == 4 and argv[1] == "--validate":
            validate(argv[2], argv[3])
            return 0
        if len(argv) == 5 and argv[1] == "--validate" and argv[4] == "--pre-publication":
            validate(argv[2], argv[3], pre_publication=True)
            return 0
        if len(argv) == 6 and argv[1] == "--post":
            post(argv[2], argv[3], argv[4], argv[5])
            return 0
        print(
            "usage: release-train-announce.py --validate DRAFT BODY_DIR [--pre-publication] | "
            "--check-token | --post VERSION DRAFT RECEIPT BODY_DIR | --record-latency TAG RECEIPT DRAFT",
            file=sys.stderr,
        )
        return 2
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
