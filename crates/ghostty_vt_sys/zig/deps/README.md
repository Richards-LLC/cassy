# Vendored uucode

`uucode-31655fba3c638229989cc524363ef5e3c7b580c1.tar.gz` is the unchanged
archive downloaded from
<https://deps.files.ghostty.org/uucode-31655fba3c638229989cc524363ef5e3c7b580c1.tar.gz>.
It corresponds to upstream revision `31655fba3c638229989cc524363ef5e3c7b580c1`.

- Archive: 2,103,371 bytes; SHA-256
  `4b3a581a1806e01e8bb9ff1e4f7e6c28ba1b0b18e6c04ba8d53c24d237aef60e`.
- Extracted regular files: 59, totaling 6,453,227 bytes.
- Zig package hash:
  `uucode-0.1.0-ZZjBPicPTQDlG6OClzn2bPu7ICkkkyWrTB6aRsBr-A1E`.
- Package fingerprint: `0x8d4ebdea3ec19865`.
- License: MIT, copyright 2025 Jacob Sandlund. The archive retains all
  upstream notices, including MIT attribution for Bjoern Hoehrmann and
  Unicode License V3. The main license and those notices are also available
  here as plain text; upstream resource attribution remains inside the archive.

Cargo's build script imports this archive with `zig fetch` into its own
`OUT_DIR/zig-global-cache`. The original URL, hash and fingerprint in
`../build.zig.zon` stay intact. `zig build --system <cache>/p` disables package
fetching and resolves the original pinned hash from those local bytes. A missing
archive or a changed package fails locally instead of contacting the URL. Local
build artifacts also live under `OUT_DIR`; no shared Zig cache is needed.

An extracted `.path` dependency would bypass Zig's package-content hash. Keeping
the archive preserves both the upstream distribution and Zig's verification,
with a smaller repository payload. To update it, download the new upstream
archive explicitly, inspect its licenses, verify its Zig hash in a disposable
cache, and update the archive, manifest pin, build-script filename and this
provenance record together.

## Acquisition regression (does not compile Rust or the Zig library)

From the repository root on macOS:

```sh
sandbox-exec -p '(version 1)(allow default)(deny network*)' \
  python3 scripts/check-ghostty-vendored-deps.py
```

On Linux with network-namespace permission, use
`unshare -rn python3 scripts/check-ghostty-vendored-deps.py`.
The script checks the real Zig dependency resolver with fresh temporary caches,
including missing and tampered packages. Network isolation is supplied by the
caller; the script alone is acquisition evidence, not offline compile evidence.

## Supervisor cold offline compile

With Cargo dependencies and Zig already installed, create a disposable target
directory inside the worktree and run the capped package check there.
On macOS the supervisor can use the following command, with a 120-second
timeout supplied by its process runner and output captured in the task artifacts:

```sh
mkdir -p target
proof_dir=$(mktemp -d "$PWD/target/ghostty-offline.XXXXXX")
sandbox-exec -p '(version 1)(allow default)(deny network*)' \
  env CARGO_TARGET_DIR="$proof_dir/target" \
  RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= CARGO_BUILD_JOBS=2 \
  cargo check --offline -p ghostty_vt_sys --lib
```

Linux can substitute `unshare -rn` for `sandbox-exec -p ...`. Keep the proof
directory and log for inspection. A fresh Cargo target guarantees fresh
`OUT_DIR` Zig caches and prevents a warm Rust build from skipping the script.
The network restriction covers Cargo, the build script, Zig and its children.
Compiler wrappers are disabled so an already running cache daemon cannot serve
the check from outside that network sandbox.
This compile is supervisor-owned in factory sessions.
