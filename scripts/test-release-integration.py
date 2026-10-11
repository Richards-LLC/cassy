#!/usr/bin/env python3
"""Fixture proof for release-train.sh --assemble (no Cargo or remote service)."""
import fcntl
import hashlib
import importlib.util
import json
import os
import shlex
from pathlib import Path
import subprocess
import tempfile
import unittest

TRAIN = Path(__file__).resolve().with_name("release-train.sh")
INTEGRATE = TRAIN.with_name("release-integrate.py")


class IntegrationAssembly(unittest.TestCase):
    def setUp(self):
        inherited = {key: value for key, value in os.environ.items()
                     if key.startswith("CAS_RELEASE_TRAIN_") or key == "CAS_RELEASE_RECEIPTS_RUN_DIR"}
        for key in inherited:
            os.environ.pop(key)
        self.addCleanup(os.environ.update, inherited)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        old_artifacts = os.environ.get("CAS_RELEASE_ARTIFACTS_ROOT")
        self.addCleanup(lambda: os.environ.pop("CAS_RELEASE_ARTIFACTS_ROOT", None)
                        if old_artifacts is None else os.environ.__setitem__("CAS_RELEASE_ARTIFACTS_ROOT", old_artifacts))
        os.environ["CAS_RELEASE_ARTIFACTS_ROOT"] = str(Path(self.temp.name) / "artifacts")
        self.root = Path(self.temp.name) / "project"
        self.root.mkdir()
        self.git("init", "-b", "main")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "Release Fixture")
        self.git("config", "commit.gpgsign", "false")
        (self.root / ".gitignore").write_text(".cas/\n")
        self.git("add", ".")
        self.git("commit", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/main", self.base)
        self.git("checkout", "-b", "epic/one")
        (self.root / "feature").write_text("one\n")
        self.git("add", ".")
        self.git("commit", "-m", "feature")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("branch", "integration/project")
        self.git("checkout", "-b", "release/test", "main")
        self.receipt_path = self.root / ".cas/merge-sweeps/integration.json"
        self.receipt_path.parent.mkdir(parents=True)
        self.receipt = {"status": "PASSED", "base": self.base, "tip": self.tip,
                        "epics": [{"id": "one", "branch": "epic/one", "tip": self.tip}]}
        self.save()

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args],
                                       stderr=subprocess.DEVNULL, text=True).strip()

    def save(self):
        self.receipt_path.write_text(json.dumps(self.receipt))

    def assemble(self):
        return subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--assemble"],
                              capture_output=True, text=True)

    def resume_action(self, action, run_dir):
        return subprocess.run(["python3", str(INTEGRATE), str(self.root), action],
                              env={**os.environ, "CAS_RELEASE_TRAIN_RUN_DIR": str(run_dir)},
                              capture_output=True, text=True)

    def advance_integration(self):
        self.git("checkout", "epic/one")
        (self.root / "feature").write_text("gate fixed\n")
        self.git("add", "feature")
        self.git("commit", "-m", "fix gate")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/heads/integration/project", self.tip)
        self.receipt.update(tip=self.tip, epics=[{"branch": "epic/one", "tip": self.tip}])
        self.save()
        self.git("checkout", "release/test")

    def prepare_resume(self):
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        run_dir = self.root / ".cas/release-run"
        run_dir.mkdir()
        (run_dir / "stage.assemble.done").write_text(self.git("rev-parse", "HEAD"))
        (run_dir / "stage.prep.done").write_text(self.git("rev-parse", "HEAD"))
        (run_dir / "stage.gate.done").write_text(self.git("rev-parse", "HEAD"))
        (run_dir / "gate.done").write_text("0\n")
        result = self.resume_action("--record-input", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        (self.root / "CHANGELOG.md").write_text("release draft preserved\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "release prose")
        return run_dir

    def test_resume_invalidates_suffix_and_rebases_release_prose(self):
        run_dir = self.prepare_resume()
        self.advance_integration()
        result = self.resume_action("--resume-check", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Integration input changed", result.stdout)
        self.assertFalse((run_dir / "stage.assemble.done").exists())
        self.assertFalse((run_dir / "stage.prep.done").exists())
        self.assertFalse((run_dir / "stage.gate.done").exists())
        self.assertFalse((run_dir / "gate.done").exists())
        self.assertTrue(list((run_dir / "superseded").glob("*/stage.assemble.done")))
        result = self.resume_action("assemble", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        self.assertEqual((self.root / "CHANGELOG.md").read_text(), "release draft preserved\n")
        self.assertEqual((self.root / "feature").read_text(), "gate fixed\n")

    def test_legacy_resume_infers_consumed_code_tip(self):
        run_dir = self.prepare_resume()
        (run_dir / "assemble.integration.json").unlink()
        # An old assemble stage might already have rebased release prose.
        (run_dir / "stage.assemble.done").write_text(self.git("rev-parse", "HEAD"))
        self.advance_integration()
        result = self.resume_action("--resume-check", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.resume_action("assemble", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        self.assertEqual((self.root / "CHANGELOG.md").read_text(), "release draft preserved\n")

    def test_resume_rejects_source_changes_without_mutating_checkout_or_receipts(self):
        run_dir = self.prepare_resume()
        (self.root / "unreviewed-source").write_text("not release metadata\n")
        self.git("add", "unreviewed-source")
        self.git("commit", "-m", "source edit")
        original = self.git("rev-parse", "HEAD")
        self.advance_integration()
        result = self.resume_action("--resume-check", run_dir)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("BLOCKER integration-release-metadata", result.stderr)
        self.assertIn(" && ", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), original)
        self.assertTrue((run_dir / "stage.assemble.done").exists())

    def test_unchanged_input_keeps_receipts_and_pending_input_refuses(self):
        run_dir = self.prepare_resume()
        result = self.resume_action("--resume-check", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((run_dir / "stage.gate.done").exists())
        self.receipt["status"] = "RUNNING"
        self.save()
        result = self.resume_action("--resume-check", run_dir)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no passing sweep", result.stderr)
        self.assertTrue((run_dir / "stage.gate.done").exists())

    def test_conflicting_metadata_rebase_restores_original_checkout(self):
        self.git("checkout", "epic/one")
        (self.root / "CHANGELOG.md").write_text("baseline\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "baseline prose")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/heads/integration/project", self.tip)
        self.receipt.update(tip=self.tip, epics=[{"branch": "epic/one", "tip": self.tip}])
        self.save()
        self.git("checkout", "release/test")
        run_dir = self.prepare_resume()
        original = self.git("rev-parse", "HEAD")
        self.git("checkout", "epic/one")
        (self.root / "CHANGELOG.md").write_text("conflicting epic prose\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "change same prose")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/heads/integration/project", self.tip)
        self.receipt.update(tip=self.tip, epics=[{"branch": "epic/one", "tip": self.tip}])
        self.save()
        self.git("checkout", "release/test")
        self.assertEqual(self.resume_action("--resume-check", run_dir).returncode, 0)
        result = self.resume_action("assemble", run_dir)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr + self.git("log", "--all", "--oneline", "--graph"))
        self.assertIn("checkout restored. Recovery:", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), original)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertFalse((self.root / ".git/rebase-merge").exists())

    def test_prep_versions_and_ledger_replay_without_replaying_source(self):
        self.git("checkout", "epic/one")
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["member"]\n')
        (self.root / "member").mkdir()
        manifest = self.root / "member/Cargo.toml"
        manifest.write_text('[package]\nname = "member"\nversion = "1.0.0"\n')
        lock = self.root / "Cargo.lock"
        lock.write_text('version = 4\n[[package]]\nname = "member"\nversion = "1.0.0"\n')
        ledger = self.root / "cas-cli/src/builtins/reference-history.json"
        ledger.parent.mkdir(parents=True)
        ledger.write_text('{}\n')
        self.git("add", ".")
        self.git("commit", "-m", "workspace source")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/heads/integration/project", self.tip)
        self.receipt.update(tip=self.tip, epics=[{"branch": "epic/one", "tip": self.tip}])
        self.save()
        self.git("checkout", "release/test")
        run_dir = self.root / ".cas/release-run"
        run_dir.mkdir()
        # Call the Git consumer directly; no proof/Cargo subprocess is allowed.
        self.assertEqual(self.resume_action("assemble", run_dir).returncode, 0)
        self.assertEqual(self.resume_action("--record-input", run_dir).returncode, 0)
        (run_dir / "stage.assemble.done").write_text(self.git("rev-parse", "HEAD"))
        for path in (manifest, lock):
            path.write_text(path.read_text().replace('"1.0.0"', '"2.0.0"'))
        ledger.write_text('{"release": true}\n')
        self.git("add", ".")
        self.git("commit", "-m", "release prep and ledger")
        self.advance_integration()
        self.assertEqual(self.resume_action("--resume-check", run_dir).returncode, 0)
        result = self.resume_action("assemble", run_dir)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        self.assertIn('"2.0.0"', manifest.read_text())
        self.assertIn('"2.0.0"', lock.read_text())
        self.assertEqual(ledger.read_text(), '{"release": true}\n')
        self.assertEqual((self.root / "feature").read_text(), "gate fixed\n")

    def delivery_lock_path(self):
        common = (self.root / ".git").resolve()
        key = hashlib.sha256(b"cas-0a21/delivery-target-lock/v1\0" + os.fsencode(common)
                             + b"\0integration/project").hexdigest()
        return self.root / ".cas/locks/delivery-target" / (key + ".lock")

    def install_recovery_stub(self, needs_lock=False):
        stub = self.root / ".cas/fake-cas"
        # The real integration-recover takes the same delivery-target lock the
        # assembler uses; a stub that needs it fails fast instead of waiting.
        lock_probe = "" if not needs_lock else f"""exec 9>>'{self.delivery_lock_path()}'
if ! python3 -c 'import fcntl; fcntl.flock(9, fcntl.LOCK_EX | fcntl.LOCK_NB)' 2>/dev/null; then
    echo 'delivery-target lock held during recovery' >&2
    exit 75
fi
printf 'acquired\\n' > .cas/recovery-lock
"""
        stub.write_text("""#!/bin/sh
set -eu
test \"$1\" = factory
test \"$2\" = integration-recover
test \"$3\" = --base-only
test \"$4\" = --release-epics
test \"$5\" = one
""" + lock_probe + """printf '%s\\n' \"${CAS_FACTORY_SESSION:-missing}\" > .cas/recovery-session
printf '%s\\n' \"${CAS_AGENT_ID:-missing}|${CAS_SESSION_ID:-missing}|${CAS_AGENT_NAME:-missing}|${CAS_AGENT_ROLE:-missing}\" > .cas/recovery-identity
base=$(git rev-parse refs/remotes/origin/main)
git update-ref refs/heads/integration/project \"$base\"
python3 - \"$base\" <<'PY'
import json
from pathlib import Path
import sys
path = Path('.cas/merge-sweeps/integration.json')
receipt = json.loads(path.read_text())
receipt['base'] = sys.argv[1]
receipt['tip'] = sys.argv[1]
receipt['status'] = 'PASSED'
receipt['epics'] = []
receipt['detail'] = 'base-only recovery passed'
path.write_text(json.dumps(receipt))
Path('.cas/healed').write_text('yes\\n')
PY
""")
        stub.chmod(0o755)
        return stub

    def refused(self, text):
        result = self.assemble()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(text, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.base)

    def test_renamed_checkout_adopts_legacy_branch(self):
        renamed = self.root.with_name("renamed-checkout")
        self.root.rename(renamed)
        self.root = renamed
        self.receipt_path = renamed / ".cas/merge-sweeps/integration.json"
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("config", "--local", "cas.integrationBranch"), "integration/project")
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)

    def test_multiple_legacy_branches_adopt_the_one_at_the_receipt_tip(self):
        # cas-52de: several integration/ branches and no config refused assembly.
        self.git("branch", "integration/old-release", self.base)
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Adopted integration/project", result.stderr)
        self.assertEqual(self.git("config", "--local", "cas.integrationBranch"), "integration/project")
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)

    def test_multiple_legacy_branches_without_a_unique_receipt_match_still_refuse(self):
        self.git("branch", "integration/twin", self.tip)
        self.refused("Multiple legacy integration branches")

    def test_release_docs_already_on_the_integration_tip_are_treated_as_applied(self):
        # cas-52de: docs copied onto the release branch in steps conflicted
        # with the integration tip, which already carried their final content.
        self.git("checkout", "epic/one")
        (self.root / "CHANGELOG.md").write_text("# Changelog\n\n## [0.0.0] - final\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "final changelog on the epic")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/heads/integration/project", self.tip)
        self.receipt.update(tip=self.tip, epics=[{"branch": "epic/one", "tip": self.tip}])
        self.save()
        self.git("checkout", "release/test")
        for step in ("## [0.0.0] - draft\n", "## [0.0.0] - final\n"):
            (self.root / "CHANGELOG.md").write_text("# Changelog\n\n" + step)
            self.git("add", "CHANGELOG.md")
            self.git("commit", "-m", "copy changelog step")
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already on the integration tip", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_from_main_assembles_without_sweep_and_records_reason(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.receipt_path.unlink()
        result = subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--assemble", "--from-main"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)
        self.assertFalse(self.receipt_path.exists(), "main mode must not forge a passing sweep")
        run = Path(os.environ["CAS_RELEASE_ARTIFACTS_ROOT"]) / "v0.0.0-project"
        receipt = json.loads((run / "assemble.integration.json").read_text())
        self.assertEqual(receipt["mode"], "from-main")
        self.assertEqual(receipt["tip"], self.tip)
        self.assertIn("already merged", receipt["reason"])
        self.assertTrue(receipt["full_gate_required"])

    def test_from_main_pipeline_still_requires_full_gate(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.receipt_path.unlink()
        result = subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--assemble", "--from-main"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--pipeline"],
                                capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("GATE_NOT_GREEN", result.stdout + result.stderr)

    def test_from_main_resume_invalidates_old_gate_when_main_moves(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        run = Path(os.environ["CAS_RELEASE_ARTIFACTS_ROOT"]) / "v0.0.0-project"
        result = subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--assemble", "--from-main"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        (run / "stage.assemble.done").write_text(self.tip)
        (run / "stage.gate.done").write_text(self.tip)
        (run / "gate.full.sha").write_text(self.tip)
        (self.root / "CHANGELOG.md").write_text("release prose\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "release prose")
        self.advance_integration()
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        result = self.resume_action("--resume-check", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((run / "gate.full.sha").exists())
        result = self.resume_action("assemble", run)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        self.assertEqual((self.root / "CHANGELOG.md").read_text(), "release prose\n")

    def assemble_from_main(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.receipt_path.unlink()
        result = subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--assemble", "--from-main"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return Path(os.environ["CAS_RELEASE_ARTIFACTS_ROOT"]) / "v0.0.0-project"

    def recording_gate(self):
        """A stand-in release gate that records its arguments and reuse env."""
        record = Path(self.temp.name) / "gate-record.json"
        gate = Path(self.temp.name) / "fake-release-gate.sh"
        script = (
            "import json, os, sys\n"
            "json.dump({'args': sys.argv[1:],\n"
            "           'cache_dir': os.environ.get('CAS_RELEASE_GATE_CACHE_DIR'),\n"
            "           'no_reuse': os.environ.get('CAS_RELEASE_GATE_NO_REUSE')},\n"
            f"          open({str(record)!r}, 'w'))\n")
        gate.write_text("#!/usr/bin/env bash\npython3 - \"$@\" <<'PY'\n" + script + "PY\n")
        gate.chmod(0o755)
        return gate, record

    def run_gate(self, *extra, internal=False):
        gate, record = self.recording_gate()
        env = {**os.environ, "CAS_RELEASE_TRAIN_GATE_CMD": str(gate)}
        if internal:
            # The --cut gate stage invokes the train exactly like this (gate.sh).
            env.update({"CAS_RELEASE_TRAIN_INVOCATION_KIND": "internal",
                        "CAS_RELEASE_TRAIN_STAGE": "gate"})
        result = subprocess.run([str(TRAIN), "0.0.0", str(self.root), "--gate", *extra],
                                capture_output=True, text=True, env=env)
        return result, record

    def wait_for_gate(self, run, record):
        import time
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not (run / "gate.done").exists():
            time.sleep(0.05)
        log = (run / "gate.log").read_text() if (run / "gate.log").exists() else ""
        self.assertEqual((run / "gate.done").read_text().strip(), "0", log)
        return json.loads(record.read_text())

    def test_from_main_refuses_gate_reuse(self):
        run = self.assemble_from_main()
        result, record = self.run_gate("--reuse")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("--gate --reuse refused", result.stderr)
        self.assertIn("from-main", result.stderr)
        self.assertFalse(record.exists(), "no gate may start")
        self.assertFalse((run / "gate.done").exists())

    def test_from_main_refuses_gate_only(self):
        run = self.assemble_from_main()
        result, record = self.run_gate("--only", "nextest")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("--gate --only refused", result.stderr)
        self.assertFalse(record.exists(), "no gate may start")
        self.assertFalse((run / "diagnostics").exists())

    def test_from_main_cut_gate_stage_runs_every_row_fresh(self):
        run = self.assemble_from_main()
        result, record = self.run_gate(internal=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("every row runs fresh", result.stdout)
        recorded = self.wait_for_gate(run, record)
        self.assertEqual(recorded["args"], ["0.0.0"], "no --reuse or --only")
        self.assertIsNone(recorded["cache_dir"], "the row cache is withheld")
        self.assertEqual(recorded["no_reuse"], "1")

    def test_sweep_assembly_gate_keeps_reuse(self):
        # Control: an ordinary sweep-backed assembly keeps its reuse paths.
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        run = Path(os.environ["CAS_RELEASE_ARTIFACTS_ROOT"]) / "v0.0.0-project"
        result, record = self.run_gate("--reuse")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        recorded = self.wait_for_gate(run, record)
        self.assertEqual(recorded["args"], ["0.0.0", "--reuse"])
        self.assertEqual(recorded["cache_dir"], str(run / "row-cache"))
        self.assertIsNone(recorded["no_reuse"])

    def test_self_heal_refuses_legacy_epics_without_ids(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.receipt["epics"][0].pop("id")
        self.save()
        result = self.assemble()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("lacks release epic IDs", result.stderr)
        self.assertFalse((self.root / ".cas/healed").exists())

    def test_clean_tip_is_consumed_via_train_action(self):
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)
        self.assertEqual(result.stdout.splitlines()[0], "PASS release assembly")
        self.assertIn(self.tip, result.stdout)

    def test_red_or_pending_sweep_refuses_old_tip(self):
        for status in ["CONFLICT", "FAILED", "RUNNING", "DEFERRED"]:
            self.receipt["status"] = status
            self.save()
            self.refused("no passing sweep")

    def test_changed_epic_refuses_stale_union(self):
        self.git("update-ref", "refs/heads/epic/one", self.base)
        self.refused("epic changed")

    def test_newer_remote_epic_is_used_over_stale_local_ref(self):
        self.git("update-ref", "refs/remotes/origin/epic/one", self.tip)
        self.git("update-ref", "refs/heads/epic/one", self.base)
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)

    def test_changed_main_heals_stale_union_once(self):
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.install_recovery_stub()
        run_dir = self.root / ".cas/release-run"
        run_dir.mkdir(parents=True)
        (run_dir / "run.env").write_text(
            "factory_session=fixture-session\n"
            "agent_id=fixture-agent-id\n"
            "session_id=fixture-supervisor-session\n"
            "agent_name=fixture-supervisor\n"
            "agent_role=supervisor\n"
        )
        env_names = [
            "CAS_RELEASE_TRAIN_CAS", "CAS_RELEASE_TRAIN_RUN_DIR", "CAS_FACTORY_SESSION",
            "CAS_AGENT_ID", "CAS_SESSION_ID", "CAS_AGENT_NAME", "CAS_AGENT_ROLE",
        ]
        old_env = {name: os.environ.get(name) for name in env_names}
        os.environ["CAS_RELEASE_TRAIN_CAS"] = str(self.root / ".cas/fake-cas")
        os.environ["CAS_RELEASE_TRAIN_RUN_DIR"] = str(run_dir)
        for name in env_names[2:]:
            os.environ.pop(name, None)
        try:
            result = self.assemble()
        finally:
            for name, value in old_env.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)
        self.assertTrue((self.root / ".cas/healed").exists())
        self.assertEqual((self.root / ".cas/recovery-session").read_text().strip(), "fixture-session")
        self.assertEqual(
            (self.root / ".cas/recovery-identity").read_text().strip(),
            "fixture-agent-id|fixture-supervisor-session|fixture-supervisor|supervisor",
        )

    def test_heal_runs_without_holding_the_delivery_target_lock(self):
        # 3.27.6: assemble held the lock while integration-recover waited on it.
        self.git("update-ref", "refs/remotes/origin/main", self.tip)
        self.install_recovery_stub(needs_lock=True)
        result = subprocess.run(
            [str(TRAIN), "0.0.0", str(self.root), "--assemble"],
            capture_output=True,
            text=True,
            env={**os.environ, "CAS_RELEASE_TRAIN_CAS": str(self.root / ".cas/fake-cas"),
                 "CAS_RELEASE_TRAIN_RECOVERY_TIMEOUT_SECS": "30"},
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("lock held during recovery", result.stderr)
        self.assertTrue((self.root / ".cas/recovery-lock").exists())
        self.assertTrue((self.root / ".cas/healed").exists())
        self.assertEqual(self.git("rev-parse", "HEAD"), self.tip)

    def test_docs_only_receipts_base_names_the_receipts_commit(self):
        self.git("checkout", "main")
        docs = self.root / "docs"
        docs.mkdir()
        (docs / "receipt.md").write_text("posted\n")
        self.git("add", "docs/receipt.md")
        self.git("commit", "-m", "docs receipt")
        docs_tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/main", docs_tip)
        self.git("checkout", "release/test")
        artifacts = Path(self.temp.name) / "artifacts"
        run_dir = artifacts / "v0.0.0-release-test"
        run_dir.mkdir(parents=True)
        (run_dir / "receipts.commit").write_text(
            f"COMMIT_SHA={docs_tip}\nBRANCH=release/test\nBASE_SHA={self.base}\n"
        )
        result = subprocess.run(
            [str(TRAIN), "0.0.0", str(self.root), "--assemble"],
            capture_output=True,
            text=True,
            env={**os.environ, "CAS_RELEASE_ARTIFACTS_ROOT": str(artifacts)},
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("only under docs/", result.stderr)
        self.assertIn(f"receipts commit {docs_tip}", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), self.base)

    def test_docs_only_release_commit_rebases_onto_integration_tip(self):
        self.git("checkout", "release/test")
        (self.root / "CHANGELOG.md").write_text("# Changelog\n\n## [0.0.0]\n")
        release_notes = self.root / "docs/release-notes"
        release_notes.mkdir(parents=True)
        (release_notes / "2099-01-01-v0.0.0-slack.md").write_text("draft\n")
        self.git("add", "CHANGELOG.md", "docs/release-notes")
        self.git("commit", "-m", "release docs")
        docs_tip = self.git("rev-parse", "HEAD")

        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        assembled_tip = self.git("rev-parse", "HEAD")
        self.assertNotEqual(assembled_tip, docs_tip)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        self.assertEqual(self.git("show", "--format=%s", "--no-patch", "HEAD"), "release docs")

    def test_changed_integration_refuses_wrong_receipt(self):
        self.git("update-ref", "refs/heads/integration/project", self.base)
        self.refused("tip changed")

    def test_release_notes_and_journey_report_rebase_together(self):
        files = {"CHANGELOG.md": "release prose\n",
                 "docs/release-notes/2099-01-01-v9.99.7-slack.md": "draft\n",
                 "docs/qa/journey-evaluations/2099-01-01-hub-web-fixture.md": "journey passed\n",
                 "docs/qa/journey-evaluations/2099-01-01-évaluation.md": "journey metadata\n"}
        for name, content in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        self.git("add", ".")
        self.git("commit", "-m", "release metadata and journey")
        result = self.assemble()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD^"), self.tip)
        for name, content in files.items():
            self.assertEqual((self.root / name).read_text(), content)

    def test_prior_receipts_and_other_paths_are_named_before_mutation(self):
        for name in ("docs/release-reports/v9.99.7.md", "unreviewed-source",
                     "docs/qa/journey-evaluations/fixture.json"):
            with self.subTest(path=name):
                self.git("reset", "--hard", self.base)
                path = self.root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("preserve me\n")
                self.git("add", ".")
                self.git("commit", "-m", "carried commit")
                original = self.git("rev-parse", "HEAD")
                result = self.assemble()
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(name, result.stderr)
                self.assertIn("prep carries the prior receipts commit", result.stderr)
                self.assertIn("start release/<ver> from origin/main", result.stderr)
                self.assertEqual(self.git("rev-parse", "HEAD"), original)
                self.assertEqual(self.git("status", "--porcelain"), "")

    def test_git_error_includes_command_exit_code_and_stderr(self):
        spec = importlib.util.spec_from_file_location("release_integrate", INTEGRATE)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with self.assertRaises(RuntimeError) as raised:
            module.git(self.root, "rev-parse", "--verify", "missing-fixture-ref")
        message = str(raised.exception)
        self.assertIn("git -C", message)
        self.assertIn("rev-parse --verify missing-fixture-ref", message)
        self.assertIn("exit 128", message)
        self.assertIn("fatal:", message)

    def test_initial_metadata_conflict_names_git_error_and_restores_checkout(self):
        self.git("checkout", "epic/one")
        (self.root / "CHANGELOG.md").write_text("epic prose\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "epic prose")
        self.tip = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/heads/integration/project", self.tip)
        self.receipt.update(tip=self.tip, epics=[{"branch": "epic/one", "tip": self.tip}])
        self.save()
        self.git("checkout", "release/test")
        (self.root / "CHANGELOG.md").write_text("release prose\n")
        self.git("add", "CHANGELOG.md")
        self.git("commit", "-m", "release prose")
        original = self.git("rev-parse", "HEAD")
        result = self.assemble()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("rebase --onto", result.stderr)
        self.assertIn("could not apply", result.stderr.lower())
        self.assertIn("checkout restored", result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), original)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertFalse((self.root / ".git/rebase-merge").exists())

    def test_resume_preserves_journey_metadata_on_updated_integration(self):
        run_dir = self.prepare_resume()
        journey = self.root / "docs/qa/journey-evaluations/fixture.md"
        journey.parent.mkdir(parents=True)
        journey.write_text("journey passed\n")
        self.git("add", ".")
        self.git("commit", "-m", "journey metadata")
        self.advance_integration()
        result = self.resume_action("--resume-check", run_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.resume_action("assemble", run_dir)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(journey.read_text(), "journey passed\n")
        self.assertEqual(self.git("rev-parse", "HEAD~2"), self.tip)

    def test_changelog_blocker_names_release_worktree_not_epic(self):
        (self.root / "CHANGELOG.md").write_text("# Release checkout without headings\n")
        script = ('set -euo pipefail\n'
                  + f'worktree={shlex.quote(str(self.root))}\nversion=9.99.7\n'
                  + f'script_dir={shlex.quote(str(TRAIN.parent))}\nrun_dir=""\n'
                  + f'source {shlex.quote(str(TRAIN.parent / "release-train.d/preflight.sh"))}\n'
                  + 'cut_preflight_check_changelog\n')
        result = subprocess.run(['/bin/bash', '-c', script], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(self.root / "CHANGELOG.md"), result.stdout + result.stderr)
        self.assertIn("this release worktree", result.stdout + result.stderr)
        self.assertEqual((self.root / "CHANGELOG.md").read_text(), "# Release checkout without headings\n")

    def test_dirty_destination_is_preserved(self):
        (self.root / "uncommitted").write_text("keep me")
        self.refused("checkout has changes")
        self.assertEqual((self.root / "uncommitted").read_text(), "keep me")

    def test_live_daemon_lock_is_nonblocking(self):
        path = self.delivery_lock_path()
        path.parent.mkdir(parents=True)
        with path.open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.refused("sweep is running")

    def test_protected_destination_is_preserved(self):
        self.git("checkout", "main")
        self.refused("detached checkout or a release/")


if __name__ == "__main__":
    unittest.main()
