#!/usr/bin/env python3
"""Plant failures in source, formerly exempt prose, and manifest additions."""
import importlib.util
import json
from pathlib import Path
import re
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("guard", ROOT / "scripts/check-violet-references.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
allowlist = json.loads(module.ALLOWLIST.read_text())
manifest = json.loads(module.MANIFEST.read_text())
pattern = re.compile(manifest["retired_reference_pattern"], re.I)
with tempfile.TemporaryDirectory(prefix="violet-reference-guard-") as directory:
    root = Path(directory)
    cases = [("src/new.rs", False), ("docs/analysis/old.md", False), ("docs/factory/data/old.csv", False), ("docs/design/old.md", False), ("docs/release-notes/new.md", False), ("docs/release-notes/new-slack.md", False), ("docs/release-notes/2026-09-29-v3.38.0-slack.md", True), ("docs/release-reports/v3.38.0.md", True), ("CHANGELOG.md", True)]
    for path, allowed in cases:
        destination = root / path; destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(manifest["retired_server"] + "\n")
        assert bool(module.violations(root, [path], allowlist, pattern)) != allowed, path
    relative = "crates/cas-types/src/violet-compatibility.json"
    destination = root / relative; destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(module.MANIFEST.read_bytes())
    assert not module.violations(root, [relative], allowlist, pattern)
    destination.write_text(destination.read_text() + manifest["retired_server"] + "\n")
    assert module.violations(root, [relative], allowlist, pattern)
print("Violet reference guard: 11 boundary cases passed")
