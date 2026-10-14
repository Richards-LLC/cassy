#!/usr/bin/env python3
"""Self-test for scripts/release_env.py (cas-9f61).

Covers the Zig discovery order, per-filesystem scratch derivation, profile
persistence and the doctor's one-fix-per-gap contract. Fake devices stand in
for real mounts, so no privileged fixture is needed.
"""

import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
import release_env  # noqa: E402

RESOLVER = SCRIPTS / "release_env.py"


def git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), "-c", "user.name=Fixture", "-c", "user.email=f@example.invalid",
                    "-c", "commit.gpgsign=false", *args], check=True, capture_output=True)


def make_exe(path, body="#!/bin/sh\necho 0.15.2\n"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o755)
    return path


class FakeHost(release_env.Host):
    """Directories under each root sit on that root's device; the rest on 99."""

    def __init__(self, roots):
        self.roots = {Path(os.path.realpath(root)): device for root, device in roots.items()}

    def _root(self, path):
        path = Path(os.path.realpath(path))
        matches = [root for root in self.roots if root == path or root in path.parents]
        return max(matches, key=lambda root: len(root.parts)) if matches else None

    def device(self, path):
        root = self._root(path)
        return self.roots[root] if root else 99

    def mount(self, path):
        return self._root(path) or Path("/")


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(os.path.realpath(self.temp.name))
        # Fixtures live under /tmp; declare a fake temporary root instead.
        self.env = {"PATH": "", "HOME": str(self.root / "home"),
                    "CAS_RELEASE_ENV_FILE": str(self.root / "release.env"),
                    "CAS_RELEASE_ENV_SCRATCH_ROOT": "",
                    "CAS_RELEASE_ENV_TEMP_ROOTS": str(self.root / "fake-tmp")}

    def tearDown(self):
        self.temp.cleanup()


class ZigOrder(Fixture):
    def setUp(self):
        super().setUp()
        self.main = self.root / "main"
        self.main.mkdir()
        git(self.main, "init", "-q", "-b", "main")
        git(self.main, "commit", "-q", "--allow-empty", "-m", "base")
        self.worktree = self.root / "worktree"
        git(self.main, "worktree", "add", "-q", "--detach", str(self.worktree))
        self.main_zig = make_exe(self.main / ".context/zig/zig")
        self.bin = self.root / "bin"
        self.path_zig = make_exe(self.bin / "zig")
        self.env["PATH"] = str(self.bin)

    def resolve(self, **kwargs):
        return release_env.resolve_zig(self.worktree, self.env, **kwargs)

    def test_exported_zig_wins_and_relative_values_resolve_against_the_checkout(self):
        custom = make_exe(self.worktree / "tools/zig")
        self.env["ZIG"] = "tools/zig"
        zig = self.resolve()
        self.assertEqual((zig.path, zig.source), (str(custom), "ZIG (environment)"))

    def test_unusable_exported_zig_falls_through_and_is_reported(self):
        self.env["ZIG"] = "/nonexistent/zig"
        zig = self.resolve()
        self.assertEqual(zig.path, str(self.main_zig))
        self.assertIn("ZIG=/nonexistent/zig is not an executable file", zig.tried)

    def test_worktree_zig_precedes_the_main_checkout(self):
        local = make_exe(self.worktree / ".context/zig/zig")
        self.assertEqual(self.resolve().path, str(local))

    def test_main_checkout_zig_is_linked_into_the_worktree_as_a_directory(self):
        zig = self.resolve()
        self.assertEqual((zig.path, zig.source), (str(self.main_zig), "main checkout .context/zig"))
        self.assertFalse((self.worktree / ".context/zig").exists(), "resolution alone must not write")
        self.resolve(link=True)
        link = self.worktree / ".context/zig"
        self.assertTrue(link.is_symlink() and link.is_dir())
        self.assertTrue(os.access(link / "zig", os.X_OK))

    def test_profile_pin_precedes_path_but_follows_checkouts(self):
        pinned = make_exe(self.root / "pinned/zig")
        self.assertEqual(self.resolve(profile={"ZIG": str(pinned)}).path, str(self.main_zig))
        self.main_zig.unlink()
        zig = self.resolve(profile={"ZIG": str(pinned)})
        self.assertEqual(zig.path, str(pinned))
        self.assertTrue(zig.source.startswith("profile ZIG"))

    def test_path_is_the_last_resort_and_a_miss_names_the_bootstrap_fix(self):
        self.main_zig.unlink()
        zig = self.resolve()
        self.assertEqual((zig.path, zig.source), (str(self.path_zig), "PATH"))
        self.path_zig.unlink()
        zig = self.resolve()
        self.assertEqual(zig.path, "")
        self.assertIn("./scripts/bootstrap-zig.sh", zig.fix)
        self.assertIn(str(self.main), zig.fix)


class ScratchPerFilesystem(Fixture):
    """Two filesystems shaped like soundwave's /home and /mnt/rewind."""

    def setUp(self):
        super().setUp()
        self.home = self.root / "home-fs"
        self.rewind = self.root / "rewind-fs"
        (self.home / "cas-release-gate").mkdir(parents=True)  # provisioned like /home/cas-release-gate
        (self.rewind / "cas/scratch").mkdir(parents=True)  # like /mnt/rewind/cas/scratch
        self.home_checkout = self.home / "user/cassy/.worktrees/release"
        self.rewind_checkout = self.rewind / "worktrees/release"
        self.home_checkout.mkdir(parents=True)
        self.rewind_checkout.mkdir(parents=True)
        self.host = FakeHost({self.home: 1, self.rewind: 2})

    def resolve(self, checkout, **kwargs):
        kwargs.setdefault("profile", {})
        return release_env.resolve_scratch(checkout, self.env, host=self.host, **kwargs)

    def test_each_filesystem_derives_its_own_base(self):
        home = self.resolve(self.home_checkout, durable=True)
        rewind = self.resolve(self.rewind_checkout, durable=True)
        self.assertEqual(home.base, str(self.home / "cas-release-gate/base"))
        self.assertEqual(rewind.base, str(self.rewind / "cas/scratch/cas-release-gate"))
        self.assertEqual((home.problem, rewind.problem), ("", ""))
        self.assertNotEqual(home.profile_key, rewind.profile_key)

    def test_create_makes_the_base_and_only_the_base(self):
        self.resolve(self.rewind_checkout, durable=True, create=True)
        self.assertTrue((self.rewind / "cas/scratch/cas-release-gate").is_dir())
        self.assertFalse((self.home / "cas/scratch").exists())

    def test_profile_pin_for_the_filesystem_wins_and_a_bind_mount_reuses_a_same_device_pin(self):
        pinned = self.rewind / "pinned"
        pinned.mkdir()
        key = release_env.profile_key(self.rewind)
        resolved = self.resolve(self.rewind_checkout, profile={key: str(pinned / "base")})
        self.assertEqual((resolved.base, resolved.source), (str(pinned / "base"), f"profile {key}"))
        # A checkout whose computed mount differs but whose device matches.
        host = FakeHost({self.home: 1, self.rewind: 2, self.root / "bind": 2})
        (self.root / "bind/qa").mkdir(parents=True)
        resolved = release_env.resolve_scratch(self.root / "bind/qa", self.env, host=host,
                                               profile={key: str(pinned / "base")}, durable=True)
        self.assertEqual(resolved.base, str(pinned / "base"))

    def test_a_legacy_single_value_on_another_filesystem_is_skipped_not_fatal(self):
        legacy = {release_env.GATE_VAR: str(self.home / "cas-release-gate/base")}
        resolved = self.resolve(self.rewind_checkout, profile=legacy, durable=True)
        self.assertEqual(resolved.base, str(self.rewind / "cas/scratch/cas-release-gate"))
        self.assertTrue(any("filesystem boundary" in line for line in resolved.rejected))

    def test_an_exported_base_is_operator_intent_and_its_problem_is_named(self):
        self.env[release_env.GATE_VAR] = str(self.home / "cas-release-gate/base")
        resolved = self.resolve(self.rewind_checkout)
        self.assertEqual(resolved.base, self.env[release_env.GATE_VAR])
        self.assertIn("filesystem boundary", resolved.problem)
        self.assertIn(f"unset {release_env.GATE_VAR}", resolved.fix)

    def test_durable_mode_refuses_temporary_roots_that_the_gate_accepts(self):
        fake_tmp = self.root / "fake-tmp"
        checkout = fake_tmp / "release"
        checkout.mkdir(parents=True)
        host = FakeHost({fake_tmp: 3})
        gate = release_env.resolve_scratch(checkout, self.env, host=host, profile={})
        self.assertEqual(gate.base, str(fake_tmp / "cas-release-gate"))
        durable = release_env.resolve_scratch(checkout, self.env, host=host, profile={}, durable=True)
        self.assertEqual(durable.base, "")
        self.assertIn("system temporary root", " ".join(durable.rejected))
        self.assertIn("cas-release-gate", durable.fix)
        self.assertIn("cas release setup", durable.fix)

    def test_a_cas_ancestor_and_an_unwritable_parent_are_refused(self):
        checkout = self.root / "other/.cas/worktrees/release"
        checkout.mkdir(parents=True)
        host = FakeHost({self.root / "other": 4})
        resolved = release_env.resolve_scratch(checkout, self.env, host=host, profile={}, durable=True)
        self.assertEqual(resolved.base, "")
        self.assertIn(".cas ancestor", " ".join(resolved.rejected))
        self.assertIn("not an existing writable directory", " ".join(resolved.rejected))

    def test_staging_scratch_root_on_the_checkout_filesystem_is_preferred(self):
        self.env["CAS_RELEASE_ENV_SCRATCH_ROOT"] = str(self.rewind / "cas/scratch")
        resolved = self.resolve(self.rewind_checkout, durable=True)
        self.assertEqual(resolved.source, "staging.scratch_root")
        resolved = self.resolve(self.home_checkout, durable=True)
        self.assertEqual(resolved.base, str(self.home / "cas-release-gate/base"))

    def test_doctor_reports_each_filesystem_once(self):
        other = self.home / "user/other"
        other.mkdir(parents=True)
        checks = release_env.check_scratch([self.home_checkout, other, self.rewind_checkout],
                                           self.env, {}, self.host)
        self.assertEqual([check.name for check in checks], [f"scratch {self.home}", f"scratch {self.rewind}"])
        self.assertEqual({check.status for check in checks}, {"warn"})  # derived, not yet pinned
        self.assertTrue(all(check.fix == "cas release setup" for check in checks))


class Profile(Fixture):
    def test_write_keeps_secrets_and_other_lines_and_replaces_only_the_managed_block(self):
        path = Path(self.env["CAS_RELEASE_ENV_FILE"])
        path.write_text("# CAS release build secrets\nexport CAS_POSTHOG_API_KEY='phc_secret'\n")
        path.chmod(0o644)
        release_env.write_profile(path, {"ZIG": "/opt/zig/zig", "CAS_RELEASE_SCRATCH_BASE_HOME": "/home/b"})
        release_env.write_profile(path, {"ZIG": "/opt/zig 2/zig"})
        text = path.read_text()
        self.assertIn("export CAS_POSTHOG_API_KEY='phc_secret'", text)
        self.assertEqual(text.count(release_env.MANAGED_BEGIN), 1)
        self.assertNotIn("CAS_RELEASE_SCRATCH_BASE_HOME", text)
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        values = release_env.read_profile(path)
        self.assertEqual(values["ZIG"], "/opt/zig 2/zig")
        self.assertEqual(values["CAS_POSTHOG_API_KEY"], "phc_secret")
        self.assertEqual(release_env.read_managed(path), {"ZIG": "/opt/zig 2/zig"})
        sourced = subprocess.run(["bash", "-c", 'set -a; . "$1"; printf %s "$ZIG"', "_", str(path)],
                                 capture_output=True, text=True, check=True)
        self.assertEqual(sourced.stdout, "/opt/zig 2/zig")

    def test_profile_keys_are_shell_names(self):
        self.assertEqual(release_env.profile_key(Path("/mnt/rewind")), "CAS_RELEASE_SCRATCH_BASE_MNT_REWIND")
        self.assertEqual(release_env.profile_key(Path("/")), "CAS_RELEASE_SCRATCH_BASE_ROOT")
        self.assertRegex(release_env.profile_key(Path("/Volumes/Data 2")), r"^[A-Z_][A-Z0-9_]*$")


class Commands(Fixture):
    """The CLI from a fresh environment: nothing exported but HOME and PATH."""

    def setUp(self):
        super().setUp()
        self.repo = self.root / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", "base")
        make_exe(self.repo / ".context/zig/zig")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for tool in ("git", "bash"):
            (self.bin / tool).symlink_to(shutil.which(tool))
        self.env["PATH"] = str(self.bin)
        self.env["CAS_RELEASE_ENV_TEMP_ROOTS"] = ""
        Path(self.env["CAS_RELEASE_ENV_FILE"]).write_text("export CAS_POSTHOG_API_KEY=phc_do_not_print\n")
        Path(self.env["CAS_RELEASE_ENV_FILE"]).chmod(0o600)

    def run_cli(self, *args):
        return subprocess.run([sys.executable, str(RESOLVER), *args, "--worktree", str(self.repo)],
                              env=self.env, capture_output=True, text=True, timeout=60)

    def test_setup_persists_discovered_values_that_later_resolution_uses(self):
        before = self.run_cli("resolve", "--durable")
        self.assertNotIn("RELEASE_ENV_SCRATCH_SOURCE='profile", before.stdout)
        result = self.run_cli("setup")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("phc_do_not_print", result.stdout + result.stderr)
        managed = release_env.read_managed(Path(self.env["CAS_RELEASE_ENV_FILE"]))
        self.assertEqual(managed["ZIG"], str(self.repo / ".context/zig/zig"))
        keys = [key for key in managed if key.startswith(release_env.PROFILE_SCRATCH_PREFIX)]
        self.assertEqual(len(keys), 1)
        self.assertTrue(Path(managed[keys[0]]).is_dir())
        script = 'eval "$("$0" "$1" resolve --worktree "$2" --durable)"; printf "%s|%s" "$RELEASE_ENV_SCRATCH" "$RELEASE_ENV_SCRATCH_SOURCE"'
        shell = subprocess.run(["bash", "-c", script, sys.executable, str(RESOLVER), str(self.repo)],
                               env=self.env, capture_output=True, text=True, check=True)
        self.assertEqual(shell.stdout, f"{managed[keys[0]]}|profile {keys[0]}")
        doctor = self.run_cli("doctor", "--json")
        scratch = [check for check in json.loads(doctor.stdout)["checks"] if check["name"].startswith("scratch")]
        self.assertEqual([check["status"] for check in scratch], ["ok"])

    def test_doctor_names_one_fix_per_gap_quickly_and_never_prints_secrets(self):
        Path(self.repo / ".context/zig/zig").unlink()
        gh = make_exe(self.bin / "gh-stub", "#!/bin/sh\necho 'not logged in' >&2\nexit 1\n")
        self.env["CAS_RELEASE_TRAIN_GH"] = str(gh)
        started = time.monotonic()
        result = self.run_cli("doctor")
        elapsed = time.monotonic() - started
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertLess(elapsed, 30)
        self.assertNotIn("phc_do_not_print", result.stdout + result.stderr)
        self.assertIn("CAS_POSTHOG_API_KEY", result.stdout)
        lines = result.stdout.splitlines()
        gaps = [index for index, line in enumerate(lines) if line.startswith("GAP ")]
        self.assertEqual({lines[index].split(":")[0] for index in gaps},
                         {"GAP  zig", "GAP  toolchain", "GAP  gh-auth"})
        for index in gaps:
            self.assertRegex(lines[index + 1], r"^\s+fix: \S", lines[index])
        self.assertRegex(lines[-1], r"release doctor: 3 gap\(s\) in \d+\.\ds")

    def test_doctor_checks_gh_auth_and_the_merge_queue_query(self):
        log = self.root / "gh.log"
        gh = make_exe(self.bin / "gh-stub", f"""#!/bin/sh
printf '%s\\n' "$*" >> {log}
case "$1" in
  auth) exit 0 ;;
  api) echo '{{"data":{{"repository":{{"mergeQueue":{{"entries":{{"nodes":[]}}}}}}}}}}' ;;
esac
""")
        self.env["CAS_RELEASE_TRAIN_GH"] = str(gh)
        checks = {check["name"]: check for check in json.loads(self.run_cli("doctor", "--json").stdout)["checks"]}
        self.assertEqual(checks["gh-auth"]["status"], "ok")
        self.assertEqual(checks["merge-queue"]["status"], "ok")
        self.assertIn('mergeQueue(branch: "main")', log.read_text())

    def test_worktrees_in_temporary_roots_are_not_release_hosts(self):
        other = self.root / "other-worktree"
        git(self.repo, "worktree", "add", "-q", "--detach", str(other))
        self.assertIn(other, release_env.worktrees(self.repo, self.env))
        self.env["CAS_RELEASE_ENV_TEMP_ROOTS"] = str(self.root)
        self.assertEqual(release_env.worktrees(self.repo, self.env), [])

    def test_missing_release_env_file_is_a_gap_with_a_fix(self):
        Path(self.env["CAS_RELEASE_ENV_FILE"]).unlink()
        checks = release_env.check_env_file(self.env)
        self.assertEqual([(check.status, bool(check.fix)) for check in checks], [("gap", True)])


class Parity(unittest.TestCase):
    def test_merge_queue_query_matches_release_train_preflight(self):
        text = (SCRIPTS / "release-train.d/preflight.sh").read_text()
        match = re.search(r"cut_preflight_merge_queue_query\(\) \{\n\s+printf '%s\\n' '([^']+)'", text)
        self.assertIsNotNone(match)
        self.assertEqual(match.group(1), release_env.MERGE_QUEUE_QUERY)

    def test_every_release_tool_asks_the_one_resolver(self):
        callers = {
            "assembly-proof.py": ["release_env.resolve_zig", "release_env.resolve_scratch"],
            "check-lane-compile.py": ["resolve_zig(repo, os.environ)"],
            "release-gate.sh": ["release_env.py\" resolve"],
            "release-train.sh": ["release_env.py\" resolve"],
            "release-train.d/preflight.sh": ["release_env.py\" resolve"],
        }
        for name, needles in callers.items():
            text = (SCRIPTS / name).read_text()
            for needle in needles:
                self.assertIn(needle, text, name)
        for name in ("release-gate.sh", "release-train.d/preflight.sh", "check-lane-compile.py"):
            self.assertNotIn('common_dir(repo).parent / ".context/zig/zig"', (SCRIPTS / name).read_text())


if __name__ == "__main__":
    unittest.main()
