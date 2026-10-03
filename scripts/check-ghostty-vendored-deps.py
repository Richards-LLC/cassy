#!/usr/bin/env python3
"""Exercise Zig's pinned package resolver without compiling the library.

Run inside sandbox-exec / unshare for no-network evidence. All caches and
tampering fixtures are disposable; the shared Zig cache is never accessed.
"""

import argparse
import hashlib
import io
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zig", default=str(root / ".context/zig/zig"))
    args = parser.parse_args()
    zig = str(Path(args.zig).resolve())
    source = root / "crates/ghostty_vt_sys/zig"
    archive = source / "deps/uucode-31655fba3c638229989cc524363ef5e3c7b580c1.tar.gz"
    manifest = (source / "build.zig.zon").read_text()
    pin = re.search(r'\.hash\s*=\s*"([^"]+)"', manifest).group(1)
    assert ".fingerprint = 0x19fa48fcc7e0b592" in manifest
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == (
        "4b3a581a1806e01e8bb9ff1e4f7e6c28ba1b0b18e6c04ba8d53c24d237aef60e"
    ), "vendored upstream archive changed"

    def run(*command):
        result = subprocess.run(
            [zig, *map(str, command)], cwd=source,
            text=True, capture_output=True, timeout=60,
        )
        return result

    def resolve(cache):
        return run(
            "build", "--fetch", "--system", cache / "p",
            "--global-cache-dir", cache, "--cache-dir", cache / "local",
        )

    with tempfile.TemporaryDirectory(prefix="ghostty-vendored-proof-") as tmp:
        tmp = Path(tmp)
        good = tmp / "good"
        fetched = run("fetch", "--global-cache-dir", good, archive)
        assert fetched.returncode == 0, fetched.stderr
        assert fetched.stdout.strip() == pin, "archive differs from manifest hash"
        resolved = resolve(good)
        assert resolved.returncode == 0, resolved.stderr
        print(f"PASS vendored archive resolves original hash {pin}")

        (tmp / "missing/p").mkdir(parents=True)
        missing = resolve(tmp / "missing")
        assert missing.returncode != 0, "missing dependency unexpectedly resolved"
        assert pin in missing.stderr, missing.stderr
        assert "unable to connect to server" not in missing.stderr, missing.stderr
        print("PASS absent local package fails without remote fallback")
        print(missing.stderr.strip())

        # Change real package content, preserving the manifest/fingerprint, so
        # zig fetch computes a different content hash rather than a tar error.
        changed = tmp / "tampered.tar.gz"
        edited = False
        with tarfile.open(archive) as original, tarfile.open(changed, "w:gz") as out:
            for member in original:
                data = original.extractfile(member).read() if member.isfile() else None
                if member.name.endswith("/build.zig"):
                    data += b"\n// tampered package fixture\n"
                    member.size = len(data)
                    edited = True
                out.addfile(member, io.BytesIO(data) if data is not None else None)
        assert edited, "tampering fixture did not change package content"
        bad = tmp / "bad"
        fetched = run("fetch", "--global-cache-dir", bad, changed)
        assert fetched.returncode == 0, fetched.stderr
        assert fetched.stdout.strip() != pin, "Zig failed to hash changed content"
        rejected = resolve(bad)
        assert rejected.returncode != 0, "tampered package unexpectedly resolved"
        assert pin in rejected.stderr, rejected.stderr
        assert "unable to connect to server" not in rejected.stderr, rejected.stderr
        print("PASS changed package fails original manifest hash selection")
        print(rejected.stderr.strip())
    print("3 dependency checks passed (compilation not exercised)")


if __name__ == "__main__":
    main()
