#!/usr/bin/env python3
"""Version identity regression for the supervisor QA candidate builder."""

import runpy
import os
import hashlib
import json
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest import mock
import unittest
from pathlib import Path


candidate = runpy.run_path(str(Path(__file__).with_name("qa-candidate-build.py")))
verify_candidate_version = candidate["verify_candidate_version"]


class CandidateVersionTests(unittest.TestCase):
    def test_real_version_output_matches_revision_and_cargo_version(self):
        revision = "e23be66725fd152c4ed16b8365afe9aa0a3d34ba"
        output = "cas 9.99.7 (e23be66 2026-10-21)"
        verify_candidate_version(output, "9.99.7", revision)
        with self.assertRaisesRegex(ValueError, "differs from revision"):
            verify_candidate_version(output, "9.99.8", revision)
        with self.assertRaisesRegex(ValueError, "differs from revision"):
            verify_candidate_version(output, "9.99.7", "f23be66725fd152c4ed16b8365afe9aa0a3d34ba")
        with self.assertRaisesRegex(ValueError, "unexpected format"):
            verify_candidate_version("cas 9.99.7", "9.99.7", revision)


    def test_candidate_target_is_owned_bounded_and_removed_after_publication(self):
        # Only the compiler and host scratch/Zig discovery are substituted.
        # Git, lifetime leases, artifact manifests and teardown remain real.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            repo, scratch, tools = root / "repo", root / "scratch/base", root / "tools"
            for path in (repo, scratch, tools):
                path.mkdir(parents=True)
            def git(*args):
                return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()
            git("init", "-q")
            git("config", "user.name", "Fixture")
            git("config", "user.email", "fixture@example.invalid")
            (repo / "cas-cli").mkdir()
            (repo / "cas-cli/Cargo.toml").write_text('[package]\nversion = "9.99.7"\n')
            git("add", ".")
            git("commit", "-qm", "fixture")
            revision = git("rev-parse", "HEAD")
            version = f"cas 9.99.7 ({revision[:7]} 2026-10-21)"
            binary = root / "binary"
            binary.write_text(f"#!{sys.executable}\n" + f"print({version!r})\n" + """
import json, os
from pathlib import Path
source = Path(json.loads(Path(os.environ['FIXTURE_TARGET_RECORD']).read_text())['target']) / 'debug/cas'
replacement = source.with_name('cas-replacement')
replacement.write_bytes(b'concurrent build replaced the source binary')
os.replace(replacement, source)
""")
            binary.chmod(0o700)
            target_record = root / "target-record"
            compiler = tools / "cargo"
            compiler.write_text(f"#!{sys.executable}\n" + """
import json, os, shutil
from pathlib import Path
out = Path(os.environ['CARGO_TARGET_DIR'])
(out / 'debug').mkdir(parents=True, exist_ok=True)
shutil.copy2(os.environ['FIXTURE_BINARY'], out / 'debug/cas')
Path(os.environ['FIXTURE_TARGET_RECORD']).write_text(json.dumps({
    'target': str(out), 'owner': (out / os.environ['FIXTURE_TARGET_OWNER']).is_file()
}))
""")
            compiler.chmod(0o700)
            legacy = repo / "target"
            legacy.mkdir()
            (legacy / "review-only").write_bytes(b"unmanaged output")
            destination = root / "artifacts/candidate"
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ.get("PATH", ""),
                       FIXTURE_BINARY=str(binary), FIXTURE_TARGET_RECORD=str(target_record),
                       FIXTURE_TARGET_OWNER=candidate['release_scratch'].OWNER)
            with mock.patch.dict(os.environ, env, clear=True), \
                 mock.patch.object(candidate['release_env'], 'resolve_scratch',
                                   return_value=SimpleNamespace(base=str(scratch), problem=None, fix='')), \
                 mock.patch.object(candidate['release_env'], 'resolve_zig',
                                   return_value=SimpleNamespace(path=str(binary), source='fixture')):
                result = candidate['build'](repo, revision, destination)
            record = json.loads(target_record.read_text())
            target = Path(record['target'])
            self.assertTrue(target.is_relative_to(scratch.parent), str(target))
            self.assertTrue(record['owner'], 'compiler target must hold bounded-cache provenance')
            self.assertFalse(target.exists(), 'candidate output must be pruned after artifact copy')
            self.assertEqual((destination / 'cas').read_bytes(), binary.read_bytes())
            self.assertEqual(result['version'], version)
            self.assertEqual(json.loads((destination / 'RECEIPT.json').read_text())['sha256'],
                             hashlib.sha256((destination / 'cas').read_bytes()).hexdigest())
            self.assertTrue((destination / 'SHA256SUMS').is_file())
            self.assertEqual((legacy / 'review-only').read_bytes(), b'unmanaged output')
            help_output = subprocess.run(
                [sys.executable, str(Path(__file__).with_name('qa-candidate-build.py')), '--help'],
                capture_output=True, text=True, check=True,
            )
            self.assertIn('--candidate-dir', help_output.stdout)
            self.assertNotIn('--task', help_output.stdout)


if __name__ == "__main__":
    unittest.main()
