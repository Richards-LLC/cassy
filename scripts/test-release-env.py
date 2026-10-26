#!/usr/bin/env python3
"""Self-test for scripts/release_env.py (cas-9f61): Zig discovery order,
per-filesystem scratch derivation and the doctor's one-fix-per-gap output.
Fake devices stand in for real mounts, so no privileged fixture is needed."""

import os
from pathlib import Path
import re
import shutil
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
    """Paths under each root sit on that root's device; everything else on 99."""

    def __init__(self, roots):
        self.roots = {Path(os.path.realpath(root)): device for root, device in roots.items()}

    def device(self, path):
        path = Path(os.path.realpath(path))
        matches = [root for root in self.roots if root == path or root in path.parents]
        return self.roots[max(matches, key=lambda root: len(root.parts))] if matches else 99


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(os.path.realpath(self.temp.name))
        # Fixtures live under /tmp; a fake temporary root stands in for it.
        self.env = {"PATH": "", "CAS_RELEASE_ENV_SCRATCH_ROOT": "", "CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT": "",
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
        self.path_zig = make_exe(self.root / "bin/zig")

    def resolve(self, **kwargs):
        return release_env.resolve_zig(self.worktree, self.env, **kwargs)

    def test_exported_zig_wins_and_resolves_relative_to_the_checkout(self):
        custom = make_exe(self.worktree / "tools/zig")
        self.env["ZIG"] = "tools/zig"
        self.assertEqual(self.resolve().path, str(custom))

    def test_unusable_exported_zig_falls_through(self):
        self.env["ZIG"] = "/nonexistent/zig"
        self.assertEqual(self.resolve().path, str(self.main_zig))

    def test_worktree_zig_precedes_the_main_checkout(self):
        local = make_exe(self.worktree / ".context/zig/zig")
        self.assertEqual(self.resolve().path, str(local))

    def test_main_checkout_zig_is_linked_into_the_worktree_as_a_directory(self):
        self.assertEqual(self.resolve().path, str(self.main_zig))
        self.assertFalse((self.worktree / ".context/zig").exists(), "resolution alone must not write")
        self.resolve(link=True)
        link = self.worktree / ".context/zig"
        self.assertTrue(link.is_symlink() and link.is_dir() and os.access(link / "zig", os.X_OK))

    def test_path_precedes_checkouts_and_a_miss_names_bootstrap(self):
        local = make_exe(self.worktree / ".context/zig/zig")
        self.env["PATH"] = str(self.root / "bin")
        self.assertEqual(self.resolve().path, str(self.path_zig))
        self.path_zig.write_text("#!/bin/sh\necho 0.16.0\n")
        rejected = self.resolve()
        self.assertEqual(rejected.path, str(local))
        self.assertTrue(any(str(self.path_zig) in reason and "0.16.0" in reason for reason in rejected.tried))
        self.path_zig.unlink()
        self.assertEqual(self.resolve().path, str(local))
        local.unlink()
        self.main_zig.unlink()
        zig = self.resolve()
        self.assertEqual(zig.path, "")
        self.assertIn(f"cd {self.main} && ./scripts/bootstrap-zig.sh", zig.fix)


class ScratchPerFilesystem(Fixture):
    """Two filesystems shaped like soundwave's /home and /mnt/rewind."""

    def setUp(self):
        super().setUp()
        self.home, self.rewind = self.root / "home-fs", self.root / "rewind-fs"
        (self.home / "cas-release-gate").mkdir(parents=True)  # like /home/cas-release-gate
        (self.rewind / "cas/scratch").mkdir(parents=True)  # like /mnt/rewind/cas/scratch
        self.home_checkout = self.home / "user/cassy/.worktrees/release"
        self.rewind_checkout = self.rewind / "worktrees/release"
        self.home_checkout.mkdir(parents=True)
        self.rewind_checkout.mkdir(parents=True)
        self.host = FakeHost({self.home: 1, self.rewind: 2})

    def resolve(self, checkout, **kwargs):
        return release_env.resolve_scratch(checkout, self.env, host=self.host, durable=True, **kwargs)

    def test_each_filesystem_derives_its_own_base_and_creates_it_on_demand(self):
        home = self.resolve(self.home_checkout, create=True)
        rewind = self.resolve(self.rewind_checkout, create=True)
        self.assertEqual((home.base, home.problem), (str(self.home / "cas-release-gate/base"), ""))
        self.assertEqual((rewind.base, rewind.problem), (str(self.rewind / "cas/scratch/cas-release-gate"), ""))
        self.assertTrue(Path(home.base).is_dir() and Path(rewind.base).is_dir())

    def test_staging_scratch_root_is_used_only_on_its_own_filesystem(self):
        self.env["CAS_RELEASE_ENV_SCRATCH_ROOT"] = str(self.rewind / "cas/scratch")
        self.assertEqual(self.resolve(self.rewind_checkout).source, "staging.scratch_root")
        self.assertEqual(self.resolve(self.home_checkout).base, str(self.home / "cas-release-gate/base"))

    def test_an_exported_base_on_another_filesystem_is_kept_and_its_problem_named(self):
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
        gate = release_env.resolve_scratch(checkout, self.env, host=host)
        self.assertEqual(gate.base, str(fake_tmp / "cas-release-gate"))
        durable = release_env.resolve_scratch(checkout, self.env, host=host, durable=True)
        self.assertEqual(durable.base, "")
        self.assertIn("system temporary root", durable.rejected[-1])
        self.assertIn("cas-release-gate", durable.fix)

    def test_a_cas_ancestor_is_refused(self):
        checkout = self.root / "other/.cas/worktrees/release"
        checkout.mkdir(parents=True)
        resolved = release_env.resolve_scratch(checkout, self.env, host=FakeHost({self.root / "other": 4}),
                                               durable=True)
        self.assertEqual(resolved.base, "")
        self.assertIn(".cas ancestor", resolved.rejected[-1])

    def test_doctor_reports_each_filesystem_once(self):
        other = self.home / "user/other"
        other.mkdir(parents=True)
        checks = release_env.check_scratch([self.home_checkout, other, self.rewind_checkout], self.host, self.env)
        self.assertEqual([(check.name, check.ok) for check in checks],
                         [(f"scratch {self.home}", True), (f"scratch {self.rewind}", True)])


class FactoryScratchRoot(Fixture):
    """cas-baa8: `[factory] scratch_root` relocates the release scratch base to
    <root>/<project-key>/release-gate, on a filesystem other than the checkout's."""

    def setUp(self):
        super().setUp()
        self.home, self.rewind = self.root / "home-fs", self.root / "rewind-fs"
        (self.home / "cas-release-gate").mkdir(parents=True)
        self.scratch = self.rewind / "cas"
        self.scratch.mkdir(parents=True)
        self.checkout = self.home / "user/cassy"
        (self.checkout / ".cas").mkdir(parents=True)
        self.host = FakeHost({self.home: 1, self.rewind: 2})
        self.env["CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"] = ""
        self.key = release_env.project_key(self.checkout / ".cas")
        self.base = self.scratch / self.key / "release-gate/base"

    def resolve(self, **kwargs):
        return release_env.resolve_scratch(self.checkout, self.env, host=self.host, durable=True, **kwargs)

    def test_scratch_root_project_key_matches_rust_golden_vector(self):
        # Same vector as cas_types::factory_scratch and the refresh script.
        self.assertEqual(
            release_env.project_key(Path("/srv/demo repo/.cas")),
            "demo-repo-57694f919d7a09378d7bd9dde4b21bd900b11644a97685c8a94b79ecc97046c8")

    def test_scratch_root_on_another_device_is_accepted_and_created(self):
        unset = self.resolve()
        self.assertEqual((unset.base, unset.problem), (str(self.home / "cas-release-gate/base"), ""))
        self.env["CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"] = str(self.scratch)
        resolved = self.resolve(create=True)
        self.assertEqual((resolved.base, resolved.source, resolved.problem),
                         (str(self.base), "factory.scratch_root", ""))
        self.assertTrue(self.base.is_dir())

    def test_an_exported_cross_device_base_must_sit_inside_the_scratch_root(self):
        # The gate exports the resolved base to its children, which re-check it.
        self.env["CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"] = str(self.scratch)
        (self.rewind / "elsewhere").mkdir()
        self.base.parent.mkdir(parents=True)
        self.env[release_env.GATE_VAR] = str(self.base)
        self.assertEqual(self.resolve().problem, "")
        self.env[release_env.GATE_VAR] = str(self.rewind / "elsewhere/base")
        self.assertIn("filesystem boundary", self.resolve().problem)

    def test_staging_scratch_root_on_another_device_is_still_refused(self):
        self.env["CAS_RELEASE_ENV_SCRATCH_ROOT"] = str(self.scratch)
        resolved = self.resolve()
        self.assertEqual(resolved.base, str(self.home / "cas-release-gate/base"))
        self.assertIn("filesystem boundary", resolved.rejected[0])

    def test_a_mount_below_the_scratch_root_is_refused(self):
        # Seeding hard-links the build-cache into the clone target: the base
        # must share the scratch root's filesystem, not merely sit under it.
        self.env["CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"] = str(self.scratch)
        self.base.parent.mkdir(parents=True)
        host = FakeHost({self.home: 1, self.rewind: 2, self.base.parent: 5})
        resolved = release_env.resolve_scratch(self.checkout, self.env, host=host, durable=True)
        self.assertNotEqual(resolved.base, str(self.base))
        self.assertIn("scratch root", resolved.rejected[0])

    def test_scratch_root_relocates_the_proof_build_cache(self):
        self.env["CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"] = str(self.scratch)
        self.assertEqual(release_env.build_cache_root(self.checkout, self.env),
                         self.scratch / self.key / "build-cache")
        self.env["CAS_RELEASE_ENV_FACTORY_SCRATCH_ROOT"] = ""
        self.assertEqual(release_env.build_cache_root(self.checkout, self.env), self.checkout / ".cas/build-cache")


class Doctor(Fixture):
    """The CLI from a fresh environment: nothing exported but PATH and fixtures."""

    def setUp(self):
        super().setUp()
        self.repo = self.root / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", "base")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for tool in ("git", "bash"):
            (self.bin / tool).symlink_to(shutil.which(tool))
        env_file = self.root / "release-profile"
        env_file.write_text("export CAS_POSTHOG_API_KEY=phc_do_not_print\n")
        self.env.update(PATH=str(self.bin), HOME=str(self.root), CAS_RELEASE_ENV_TEMP_ROOTS="",
                        CAS_RELEASE_ENV_FILE=str(env_file))

    def doctor(self):
        return subprocess.run([sys.executable, str(RESOLVER), "doctor", "--worktree", str(self.repo)],
                              env=self.env, capture_output=True, text=True, timeout=60)

    def test_doctor_names_one_fix_per_gap_quickly_and_never_prints_secrets(self):
        self.env["CAS_RELEASE_TRAIN_GH"] = str(make_exe(self.bin / "gh-stub", "#!/bin/sh\necho 'not logged in' >&2\nexit 1\n"))
        started = time.monotonic()
        result = self.doctor()
        self.assertLess(time.monotonic() - started, 30)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertNotIn("phc_do_not_print", result.stdout + result.stderr)
        lines = result.stdout.splitlines()
        gaps = [index for index, line in enumerate(lines) if line.startswith("GAP ")]
        self.assertEqual({lines[index].split(":")[0] for index in gaps},
                         {"GAP  zig", "GAP  toolchain", "GAP  gh-auth"})
        for index in gaps:
            self.assertRegex(lines[index + 1], r"^\s+fix: \S", lines[index])
        self.assertRegex(lines[-1], r"release doctor: 3 gap\(s\) in \d+\.\ds")

    def test_doctor_passes_gh_auth_and_the_merge_queue_query(self):
        log = self.root / "gh.log"
        self.env["CAS_RELEASE_TRAIN_GH"] = str(make_exe(self.bin / "gh-stub", f"""#!/bin/sh
printf '%s\\n' "$*" >> {log}
case "$1" in
  auth) exit 0 ;;
  api) echo '{{"data":{{"repository":{{"mergeQueue":{{"entries":{{"nodes":[]}}}}}}}}}}' ;;
esac
"""))
        make_exe(self.repo / ".context/zig/zig")
        out = self.doctor().stdout
        for line in ("ok   gh-auth", "ok   merge-queue: merge queue readable (0 entries)", "ok   zig",
                     "ok   release-env"):
            self.assertIn(line, out)
        self.assertIn('mergeQueue(branch: "main")', log.read_text())


class Parity(unittest.TestCase):
    def test_merge_queue_query_matches_release_train_preflight(self):
        text = (SCRIPTS / "release-train.d/preflight.sh").read_text()
        match = re.search(r"cut_preflight_merge_queue_query\(\) \{\n\s+printf '%s\\n' '([^']+)'", text)
        self.assertEqual(match.group(1), release_env.MERGE_QUEUE_QUERY)


if __name__ == "__main__":
    unittest.main()
