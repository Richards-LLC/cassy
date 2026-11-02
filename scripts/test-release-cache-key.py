#!/usr/bin/env python3
"""Release dependency identity follows build compatibility, not local versions."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("release_cache_key", Path(__file__).with_name("release-cache-key.py"))
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class ReleaseCacheKeyTest(unittest.TestCase):
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
