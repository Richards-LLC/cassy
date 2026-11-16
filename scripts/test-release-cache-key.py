#!/usr/bin/env python3
"""Release dependency identity follows build compatibility, not local versions."""
import importlib.util
import re
from pathlib import Path
import tempfile
import unittest
import yaml

spec = importlib.util.spec_from_file_location("release_cache_key", Path(__file__).with_name("release-cache-key.py"))
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class ReleaseCacheKeyTest(unittest.TestCase):
    def test_cache_saves_stay_in_main_scope_without_dispatch_writes(self):
        workflow = yaml.safe_load((Path(__file__).resolve().parent.parent /
                                   ".github/workflows/release-prebuild.yml").read_text())
        # env.SELF_HOSTED exists only where a job declares it (build-macos); elsewhere
        # Actions reads the unset property as empty. The self-hosted target dir is its
        # own cache, so that path never uploads target to the Actions cache.
        saves = [(step, sh) for job in workflow["jobs"].values() for step in job.get("steps", [])
                 if step.get("uses") == "actions/cache/save@v4"
                 for sh in (["true", "false"] if "SELF_HOSTED" in job.get("env", {}) else [""])]
        self.assertTrue(saves, "release prebuild must still seed caches on main")
        self.assertIn("true", {sh for _, sh in saves}, "the self-hosted macOS path must be checked")
        for step, self_hosted in saves:
            saves_target = "target" in step["with"]["path"].split()
            for event, ref, needed, hit, expected in [
                ("push", "refs/heads/main", "true", "false", True),
                ("push", "refs/heads/main", "false", "true", False),
                ("push", "refs/heads/main", "true", "true", False),
                ("workflow_dispatch", "refs/heads/main", "true", "false", False),
                ("workflow_dispatch", "refs/heads/factory/test", "true", "false", False),
                ("push", "refs/heads/factory/test", "true", "false", False),
                ("push", "refs/tags/v1.2.3", "true", "false", False),
                ("merge_group", "refs/heads/gh-readonly-queue/main/test", "true", "false", False),
            ]:
                values = {"github.event_name": event, "github.ref": ref,
                          "steps.compile.outputs.needed": needed,
                          "steps.cache-lookup.outputs.cache-hit": hit,
                          "steps.zig-cache.outputs.cache-hit": hit,
                          "env.SELF_HOSTED": self_hosted}
                expected = expected and not (self_hosted == "true" and saves_target)
                expression = re.sub(r"[\w-]+(?:\.[\w-]+)+", lambda m: repr(values[m[0]]), step["if"])
                expression = expression.replace("&&", " and ").replace("||", " or ")
                with self.subTest(step=step["name"], self_hosted=self_hosted, event=event, ref=ref, needed=needed, hit=hit):
                    self.assertEqual(eval(expression, {"__builtins__": {}}), expected)

    def test_workspace_version_churn_and_build_compatibility(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".cargo").mkdir()
            (root / "scripts").mkdir()
            (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.99.0"\n')
            (root / ".cargo/config.toml").write_text('[build]\njobs=-1\n')
            (root / "scripts/bootstrap-zig.sh").write_text('ZIG_VERSION="0.15.2"\n')
            (root / "Cargo.toml").write_text('[workspace.package]\nversion="1.0.0"\n[profile.release-fast]\nlto="thin"\n')
            lock = root / "Cargo.lock"
            original = 'version=4\n[[package]]\nname="cas"\nversion="1.0.0"\n[[package]]\nname="external"\nversion="2.0.0"\nsource="registry+https://example.test"\nchecksum="aaa"\n'
            lock.write_text(original)
            base = cache.cache_key(root, "aarch64-apple-darwin", {"ImageOS": "macos26", "CARGO_PROFILE_RELEASE_FAST_LTO": "off"})
            lock.write_text(original.replace('"1.0.0"', '"1.0.1"'))
            (root / "Cargo.toml").write_text((root / "Cargo.toml").read_text().replace('"1.0.0"', '"1.0.1"'))
            self.assertEqual(base, cache.cache_key(root, "aarch64-apple-darwin", {"ImageOS": "macos26", "CARGO_PROFILE_RELEASE_FAST_LTO": "off"}))
            lock.write_text(original.replace('"2.0.0"', '"2.1.0"'))
            self.assertNotEqual(base, cache.cache_key(root, "aarch64-apple-darwin", {"ImageOS": "macos26", "CARGO_PROFILE_RELEASE_FAST_LTO": "off"}))
            lock.write_text(original)
            for target, env in [("x86_64-unknown-linux-gnu", {"ImageOS": "macos26", "CARGO_PROFILE_RELEASE_FAST_LTO": "off"}),
                                ("aarch64-apple-darwin", {"ImageOS": "macos26", "CARGO_PROFILE_RELEASE_FAST_LTO": "thin"})]:
                self.assertNotEqual(base, cache.cache_key(root, target, env))
            (root / "rust-toolchain.toml").write_text('[toolchain]\nchannel="1.99.1"\n')
            self.assertNotEqual(base, cache.cache_key(root, "aarch64-apple-darwin", {"ImageOS": "macos26", "CARGO_PROFILE_RELEASE_FAST_LTO": "off"}))


if __name__ == "__main__":
    unittest.main()
