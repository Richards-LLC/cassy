#!/usr/bin/env python3
"""Plant failures in source, formerly exempt prose, and manifest additions."""
import importlib.util
import json
from pathlib import Path
import re
import tempfile
import subprocess
import os
import yaml

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

# Execute the relocated admission step against governed Rust fixtures. The
# queue must retain the guard self-tests, but must not repeat the content scan.
workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
steps = workflow['jobs']['ci-diff']['steps']
scan = [step for step in steps if 'test-no-mecha-references.sh' in step.get('run', '')]
assert len(scan) == 1, 'reference scan must have one admission owner'
assert scan[0].get('if') == "github.event_name != 'merge_group'", 'scan belongs before queue admission'
recipe = (ROOT / 'cas-cli/Makefile').read_text().split('test-ci-tiers:', 1)[1].split('\n\n', 1)[0]
assert 'bash scripts/test-no-mecha-references.sh' not in recipe, 'queue repeats repository content scan'
assert 'python3 scripts/test-violet-reference-guard.py' in recipe, 'keep distinct guard self-tests'
with tempfile.TemporaryDirectory(prefix='violet-admission-') as directory:
    root = Path(directory)
    subprocess.run(['git', 'init', '-q', root], check=True)
    for relative in ('scripts/test-no-mecha-references.sh', 'scripts/check-violet-references.py',
                     'scripts/violet-reference-allowlist.json', 'crates/cas-types/src/violet-compatibility.json'):
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((ROOT / relative).read_bytes())
    target = root / 'cas-cli/src/fixture.rs'
    target.parent.mkdir(parents=True)
    target.write_text(manifest['retired_server'] + '\n')
    result = subprocess.run(['bash', '-e', '-c', scan[0]['run']], cwd=root, capture_output=True, text=True)
    assert result.returncode != 0 and 'cas-cli/src/fixture.rs:1:' in result.stderr, result
    target.write_text('// clean fixture\n')
    result = subprocess.run(['bash', '-e', '-c', scan[0]['run']], cwd=root, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
print('Violet admission: source failure and clean control passed; no late queue scan')

rollup = workflow['jobs']['fast-validation']
assert "needs.ci-diff.result != 'success'" in rollup['if'], 'docs class must not hide admission failure'
admit = next(s for s in rollup['steps'] if s.get('env', {}).get('CI_DIFF'))
for state, expected in [('failure', False), ('cancelled', False), ('success', True)]:
    result = subprocess.run(['bash', '-e', '-c', admit['run']],
                            env=dict(os.environ, CI_DIFF=state), capture_output=True)
    assert (result.returncode == 0) == expected, state
print('Violet admission: required rollup refuses failed/cancelled scan and accepts PASS')
