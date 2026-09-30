#!/usr/bin/env python3
"""Real Git/archive/executable regressions for the rule-175 completion gate."""
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent


def run(*args, cwd=None, env=None):
    return subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, check=True).stdout.strip()


def executable(path, text):
    path.write_text(text)
    path.chmod(0o755)
    return path


class Fixture:
    def __init__(self, root):
        self.root = root
        self.worktree = root / "worktree"
        self.worktree.mkdir()
        self.receipts = root / "receipts"
        self.receipts.mkdir()
        self.remote = root / "remote.git"
        run("git", "init", "--bare", str(self.remote))
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Release fixture")
        self.git("config", "user.email", "fixture@example.test")
        self.git("remote", "add", "origin", str(self.remote))
        (self.worktree / "cas-cli").mkdir()
        (self.worktree / "cas-cli/Cargo.toml").write_text('[package]\nversion = "1.0.0"\n')
        self.commit("previous release")
        self.git("tag", "v1.0.0")
        (self.worktree / "first-change").write_text("user-facing change one")
        self.first = self.commit("session change one")
        (self.worktree / "second-change").write_text("user-facing change two")
        (self.worktree / "cas-cli/Cargo.toml").write_text('[package]\nversion = "1.0.1"\n')
        self.landed = self.commit("session change two and version bump")
        self.git("tag", "v1.0.1")
        self.git("push", "origin", "main", "--tags")
        self.binary = executable(root / "host-cas", f'#!/bin/sh\necho "cas 1.0.1 ({self.landed[:7]} 2099-01-01)"\n')
        self.prefix = "MACOS" if platform.system() == "Darwin" else "LINUX"
        triple = "aarch64-apple-darwin" if self.prefix == "MACOS" else "x86_64-unknown-linux-gnu"
        self.archive = root / f"cas-{triple}.tar.gz"
        self.write_archive(self.binary.read_bytes())
        sha = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        self.publication = {"TAG": "v1.0.1", "PUBLISHED_AT": "2099-01-01T00:00:00Z",
                            "LINUX_ASSET": "cas-x86_64-unknown-linux-gnu.tar.gz", "LINUX_SHA256": "b" * 64,
                            "MACOS_ASSET": "cas-aarch64-apple-darwin.tar.gz", "MACOS_SHA256": "b" * 64}
        self.publication[f"{self.prefix}_SHA256"] = sha
        self.live = {"isDraft": False, "publishedAt": self.publication["PUBLISHED_AT"],
                     "assets": [{"name": self.publication[f"{p}_ASSET"],
                                 "digest": "sha256:" + self.publication[f"{p}_SHA256"]}
                                for p in (self.prefix, "MACOS" if self.prefix == "LINUX" else "LINUX")]}
        self.workflow = {"headSha": self.landed, "headBranch": "v1.0.1",
                         "status": "completed", "conclusion": "success"}
        self.host = {"status": "PASS", "version": "1.0.1", "cas_version": "1.0.1",
                     "hub_running": True, "hub_version": "1.0.1", "refresh_binary_version": "1.0.1"}
        self.gh = executable(root / "gh", '''#!/usr/bin/env python3
import json, os, pathlib, shutil, subprocess, sys
root = pathlib.Path(__file__).parent
if sys.argv[1:3] == ['release', 'view']:
    print((root / 'live.json').read_text())
elif sys.argv[1:3] == ['release', 'download']:
    shutil.copyfile(root / sys.argv[sys.argv.index('--pattern') + 1],
                    pathlib.Path(sys.argv[sys.argv.index('--dir') + 1]) / sys.argv[sys.argv.index('--pattern') + 1])
    if os.environ.get('FAKE_GH_ADVANCE_MAIN'):
        worktree = root / 'worktree'
        (worktree / 'during-install').write_text('late main merge')
        for args in (['add', '.'], ['commit', '-m', 'merge during install'], ['push', 'origin', 'main']):
            subprocess.run(['git', '-C', str(worktree), *args], check=True, capture_output=True)
else:
    raise SystemExit(65)
''')
        self.env = dict(os.environ, CAS_RELEASE_TRAIN_GH=str(self.gh), CAS_RELEASE_TRAIN_CAS=str(self.binary))
        self.save()

    def git(self, *args):
        return run("git", "-C", str(self.worktree), *args)

    def commit(self, message):
        self.git("add", ".")
        self.git("commit", "-m", message)
        return self.git("rev-parse", "HEAD")

    def write_archive(self, data, symlink=False):
        with tarfile.open(self.archive, "w:gz") as archive:
            member = tarfile.TarInfo("cas")
            member.mode = 0o755
            if symlink:
                member.type = tarfile.SYMTYPE
                member.linkname = "/bin/true"
                archive.addfile(member)
            else:
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))

    def save(self):
        (self.receipts / "landed-main.sha").write_text(self.landed + "\n")
        (self.receipts / "release-workflow.json").write_text(json.dumps(self.workflow))
        (self.receipts / "release-published.receipt").write_text(
            "".join(f"{key}={value}\n" for key, value in self.publication.items()))
        (self.receipts / "host-update.json").write_text(json.dumps(self.host))
        (self.root / "live.json").write_text(json.dumps(self.live))

    def gate(self):
        return subprocess.run([sys.executable, str(ROOT / "release-completion.py"), "1.0.1",
                               str(self.receipts), str(self.worktree)], env=self.env,
                              capture_output=True, text=True)


class CompletionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="release-rule-175-")
        self.addCleanup(self.temporary.cleanup)
        self.f = Fixture(Path(self.temporary.name))

    def assert_blocked(self, phrase):
        result = self.f.gate()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        evidence = json.loads((self.f.receipts / "delivery-completion.json").read_text())
        self.assertEqual(evidence["status"], "FAIL")
        self.assertIn(phrase, evidence["blocker"])

    def test_complete_contains_both_session_changes_and_clean_installed_build(self):
        result = self.f.gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        evidence = json.loads((self.f.receipts / "delivery-completion.json").read_text())
        self.assertIn(self.f.first, evidence["covered_commits"])
        self.assertIn(self.f.landed, evidence["covered_commits"])
        self.assertEqual(evidence["binary_sha256"], hashlib.sha256(self.f.binary.read_bytes()).hexdigest())
        self.assertIsNone(evidence["announcement_embargo"])

    def test_audited_completion_without_runtime_release_fails(self):
        (self.f.receipts / "release-published.receipt").unlink()
        self.assert_blocked("release-published.receipt")

    def test_explicit_announcement_embargo_does_not_require_posted_receipts(self):
        self.f.env["CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO"] = "operator: no Slack until Friday"
        result = self.f.gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("no Slack", json.loads((self.f.receipts / "delivery-completion.json").read_text())["announcement_embargo"])

    def test_embargo_never_waives_publication(self):
        self.f.env["CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO"] = "operator embargo"
        self.f.live["isDraft"] = True
        self.f.save()
        self.assert_blocked("not published")

    def test_new_main_merge_missing_from_release_fails_even_after_pass(self):
        self.assertEqual(self.f.gate().returncode, 0)
        (self.f.worktree / "third-change").write_text("late session change")
        self.f.commit("another merged change")
        self.f.git("push", "origin", "main")
        self.assert_blocked("merged changes absent")

    def test_main_advancing_during_install_cannot_complete_from_old_snapshot(self):
        self.f.env["FAKE_GH_ADVANCE_MAIN"] = "1"
        self.assert_blocked("main changed during completion")

    def test_malformed_workflow_replaces_an_earlier_pass_with_failure(self):
        self.assertEqual(self.f.gate().returncode, 0)
        self.f.workflow = []
        self.f.save()
        self.assert_blocked("publication workflow")

    def test_wrong_tag_tree_fails(self):
        self.f.git("tag", "-f", "v1.0.1", self.f.first)
        self.f.git("push", "--force", "origin", "refs/tags/v1.0.1")
        self.assert_blocked("tag does not contain")

    def test_tagged_tree_not_landed_on_main_cannot_complete(self):
        self.f.git("push", "--force", "origin", f"{self.f.first}:refs/heads/main")
        self.assert_blocked("has not landed on main")

    def test_clean_install_has_no_operator_home_repository_or_release_environment(self):
        data = (f'''#!/bin/sh
[ "$1" = --version ] || exit 64
[ -z "${{CAS_RELEASE_TRAIN_CAS:-}}" ] || exit 65
[ ! -e "$HOME/.cas" ] || exit 66
[ ! -e first-change ] || exit 67
[ "$HOME/.local/bin/cas" = "$0" ] || exit 68
echo "cas 1.0.1 ({self.f.landed[:7]} 2099-01-01)"
''').encode()
        self.f.binary.write_bytes(data)
        self.f.write_archive(data)
        self.f.publication[f"{self.f.prefix}_SHA256"] = hashlib.sha256(self.f.archive.read_bytes()).hexdigest()
        self.f.live["assets"][0]["digest"] = "sha256:" + self.f.publication[f"{self.f.prefix}_SHA256"]
        self.f.save()
        result = self.f.gate()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_same_version_stale_archive_build_fails(self):
        data = self.f.binary.read_bytes().replace(self.f.landed[:7].encode(), self.f.first[:7].encode())
        self.f.write_archive(data)
        self.f.publication[f"{self.f.prefix}_SHA256"] = hashlib.sha256(self.f.archive.read_bytes()).hexdigest()
        self.f.live["assets"][0]["digest"] = "sha256:" + self.f.publication[f"{self.f.prefix}_SHA256"]
        self.f.save()
        self.assert_blocked("version/build differs")

    def test_dirty_build_fails(self):
        self.f.write_archive(self.f.binary.read_bytes().replace(self.f.landed[:7].encode(), (self.f.landed[:7] + "-dirty").encode()))
        self.f.publication[f"{self.f.prefix}_SHA256"] = hashlib.sha256(self.f.archive.read_bytes()).hexdigest()
        self.f.live["assets"][0]["digest"] = "sha256:" + self.f.publication[f"{self.f.prefix}_SHA256"]
        self.f.save()
        self.assert_blocked("version/build differs")

    def test_host_binary_replaced_after_convergence_fails(self):
        self.f.binary.write_text("#!/bin/sh\necho old-host\n")
        self.assert_blocked("host binary differs")

    def test_archive_bytes_must_match_published_digest(self):
        self.f.archive.write_bytes(b"wrong downloaded bytes")
        self.assert_blocked("archive digest differs")

    def test_live_published_assets_must_match_saved_receipt(self):
        self.f.live["assets"][0]["digest"] = "sha256:" + "c" * 64
        self.f.save()
        self.assert_blocked("asset receipt differs")

    def test_missing_update_proof_fails(self):
        self.f.host["status"] = "FAIL"
        self.f.save()
        self.assert_blocked("host install/update")

    def test_workflow_of_another_tree_fails(self):
        self.f.workflow["headSha"] = self.f.first
        self.f.save()
        self.assert_blocked("publication workflow")

    def test_duplicate_publication_fields_cannot_shadow_identity(self):
        with (self.f.receipts / "release-published.receipt").open("a") as receipt:
            receipt.write("TAG=v1.0.0\n")
        self.assert_blocked("duplicate field")

    def test_version_bump_is_mandatory(self):
        self.f.git("tag", "v1.0.2", self.f.first)
        self.assert_blocked("version was not bumped")

    def test_unavailable_previous_release_fails(self):
        self.f.git("tag", "-d", "v1.0.0")
        self.f.git("push", "origin", ":refs/tags/v1.0.0")
        self.assert_blocked("previous release tag is missing")

    def test_archive_symlink_is_not_an_install(self):
        self.f.write_archive(b"", symlink=True)
        self.f.publication[f"{self.f.prefix}_SHA256"] = hashlib.sha256(self.f.archive.read_bytes()).hexdigest()
        self.f.live["assets"][0]["digest"] = "sha256:" + self.f.publication[f"{self.f.prefix}_SHA256"]
        self.f.save()
        self.assert_blocked("regular cas binary")

    def cut(self, embargo=None):
        # Exercise the real dispatcher and completion seam. Earlier stages use
        # fixtures; no network announcement or Rust build is permitted here.
        script = r'''
set -euo pipefail
version=1.0.1
worktree="$FIXTURE_WORKTREE"
run_dir="$FIXTURE_RUN"
script_dir="$FIXTURE_SCRIPTS"
source "$script_dir/release-train.d/ledger.sh"
source "$script_dir/release-train.d/host-update.sh"
write_run_env() { :; }
cut_resume_outputs() { :; }
for stage in preflight assemble prep ledger gate pr-body pipeline publish post-publication announce report receipts; do
    name="cut_stage_${stage//-/_}"
    eval "$name() { printf '%s\\n' '$stage' >> \"\$run_dir/stages.log\"; }"
done
cut_stage_host_update() { release_train_delivery_completion; }
# The stubbed assemble stage has no integration receipt to record.
python3() {
    if [[ "${1:-}" == */release-integrate.py ]]; then return 0; fi
    command python3 "$@"
}
cut_run false
'''
        env = dict(self.f.env, FIXTURE_WORKTREE=str(self.f.worktree), FIXTURE_RUN=str(self.f.receipts),
                   FIXTURE_SCRIPTS=str(ROOT))
        if embargo is not None:
            env["CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO"] = embargo
        return subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)

    def test_embargo_cut_publishes_and_proves_install_leaves_announcements_pending(self):
        result = self.cut("operator: embargo through Friday")
        self.assertEqual(result.returncode, 0, result.stderr)
        stages = (self.f.receipts / "stages.log").read_text().splitlines()
        self.assertIn("publish", stages)
        self.assertNotIn("announce", stages)
        self.assertNotIn("report", stages)
        self.assertNotIn("receipts", stages)
        self.assertTrue((self.f.receipts / "stage.host-update.done").exists())
        self.assertFalse((self.f.receipts / "stage.announce.done").exists())
        # Omission on a resume preserves the embargo. Explicit empty lifts it.
        self.assertEqual(self.cut().returncode, 0)
        self.assertFalse((self.f.receipts / "stage.announce.done").exists())
        lifted = self.cut("")
        self.assertEqual(lifted.returncode, 0, lifted.stderr)
        self.assertTrue((self.f.receipts / "stage.announce.done").exists())

    def test_ledger_never_marks_completion_on_missing_release_even_with_embargo(self):
        (self.f.receipts / "release-published.receipt").unlink()
        result = self.cut("operator embargo")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.f.receipts / "stage.host-update.done").exists())
        self.assertNotIn("cut complete", result.stdout)

    def test_existing_host_done_marker_cannot_hide_new_unreleased_main_changes(self):
        self.assertEqual(self.cut("operator embargo").returncode, 0)
        (self.f.worktree / "new-main-change").write_text("unreleased")
        self.f.commit("late merge")
        self.f.git("push", "origin", "main")
        result = self.cut()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("merged changes absent", result.stderr)
        self.assertNotIn("cut complete", result.stdout)
        self.assertFalse((self.f.receipts / "stage.host-update.done").exists())

    def test_standalone_announce_honors_persisted_embargo_before_adapter(self):
        (self.f.receipts / "announcement-embargo.txt").write_text("operator embargo")
        marker = self.f.root / "unexpected-post"
        poster = executable(self.f.root / "poster", f'#!/bin/sh\ntouch "{marker}"\n')
        script = r'''
set -euo pipefail
run_dir="$FIXTURE_RUN"
worktree="$FIXTURE_WORKTREE"
version=1.0.1
source "$FIXTURE_SCRIPTS/release-train.d/announce.sh"
release_train_announce
'''
        result = subprocess.run(["bash", "-c", script], capture_output=True, text=True,
            env=dict(self.f.env, FIXTURE_RUN=str(self.f.receipts), FIXTURE_WORKTREE=str(self.f.worktree),
                     FIXTURE_SCRIPTS=str(ROOT), CAS_RELEASE_TRAIN_ANNOUNCE_POST_CMD=str(poster)))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("explicit announcement embargo", result.stderr)
        self.assertFalse(marker.exists())

    def test_standalone_report_honors_explicit_embargo(self):
        result = subprocess.run([str(ROOT / "release-train.sh"), "1.0.1", str(self.f.worktree), "--report"],
            capture_output=True, text=True, env=dict(self.f.env, CAS_RELEASE_ARTIFACTS_ROOT=str(self.f.root / "artifacts"),
                                                   CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO="operator embargo"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("BLOCKER report: explicit announcement embargo", result.stderr)


if __name__ == "__main__":
    unittest.main()
