#!/usr/bin/env python3
"""Compatible release dependencies shared by main seeds and queue builds.

Local package versions change on each release; external lock entries, toolchain,
profile, native tools and compiler flags define the reusable dependency identity.
Cargo still fingerprints sources after restore; cached products never authorize
adopting an archive (that requires the finder's exact SHA/tree check).
"""
import hashlib
import json
import os
from pathlib import Path
import sys
import tomllib


def cache_key(root: Path, target: str, env: dict[str, str]) -> str:
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    identity = {
        "target": target,
        "external": [{k: v for k, v in package.items() if k != "dependencies"}
                     for package in lock["package"] if "source" in package],
        "profiles": tomllib.loads((root / "Cargo.toml").read_text()).get("profile", {}),
        "files": {name: (root / name).read_text() for name in (
            "rust-toolchain.toml", ".cargo/config.toml", "scripts/bootstrap-zig.sh")},
        "env": {key: value for key, value in env.items()
                if key.startswith(("CARGO_PROFILE_", "CARGO_TARGET_", "CFLAGS", "CXXFLAGS"))
                or key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CC", "CXX", "SDKROOT",
                           "DEVELOPER_DIR", "ImageOS", "ImageVersion", "RELEASE_PROFILE")},
    }
    digest = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    return f"release-deps-v1-{target}-{digest}"


if __name__ == "__main__":
    root = Path.cwd()
    toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    print(f"toolchain={toolchain}")
    print(f"key={cache_key(root, sys.argv[1], dict(os.environ))}")
