# Codex factory startup crash: AVX-512 leaked into the CAS release binary

Date: 2026-08-08  
Status: source fix and release gates committed; `2.54.1` replacement prepared and awaiting final artifact publication proof  
Severity: high local outage, with potentially broader x86_64 release-portability impact  
Audience: CAS maintainers, release engineering, and factory/harness owners

## Impact

`cas codex` could not start a new Codex factory on the affected machine. The
factory reached **Spawning agents**, the CAS daemon terminated, and the client
then reported that the new session was not running. Repeated retries failed in
the same startup phase.

The failure was inside the installed CAS `2.54.0` binary, not in the standalone
Codex CLI. Direct Codex startup, `codex doctor`, authentication, and network
connectivity all succeeded. An already-running factory was unaffected. The
confirmed affected host is an x86_64 Linux machine with an Intel Core i9-13900K
that supports AVX and AVX2 but not AVX-512.

The observed diagnostic window ran from the first retained failed launch at
2026-08-08 21:49:57 EDT to the portable release binary completed at
2026-08-08 22:07:32 EDT: 17 minutes 35 seconds. That is not the full incident
duration; the exact onset and external blast radius were not established.

## Overview

| Field | Detail |
| --- | --- |
| User-visible symptom | `cas codex` showed **Spawning agents**, then exited with `[ERROR] Session '<name>' not found or not running` |
| First retained failure | 2026-08-08 21:49:57 EDT / 2026-08-09 01:49:57 UTC |
| Definitive detection | 2026-08-08 21:57:37 EDT, when `coredumpctl` recorded `cas` dying from signal 4 (`SIGILL`) |
| Affected installed CAS | `cas 2.54.0 (c215061 2026-08-09)` at `/home/pippenz/.local/bin/cas` |
| Codex version | `codex-cli 0.147.0`; CAS preflight said the validated harness was `0.146.0` and the installed harness was stale |
| Host | x86_64 Linux, 13th Gen Intel Core i9-13900K, 32 logical CPUs; AVX/AVX2 present, AVX-512 absent |
| Root cause | The Ghostty VT Zig dependency was built without an explicit target for native Cargo builds, allowing a release runner's AVX-512 features into the distributed static library |
| Crash instruction | `vcvttsd2usi %xmm0,%rcx`, AVX-512 encoded, at installed executable offset `0x2f24543` |
| Local mitigation | Always pass Cargo's supported target triple to Zig, rebuild CAS, and replace the installed binary |
| Local resolution | Rebuilt binary installed at 2026-08-08 22:08:03 EDT; fresh factory startup reached `READY` and `SYSTEM READY` and remained attachable |
| Upstream status | Fix, regression tests, fail-closed target policy, ISA gate, `2.54.1` version bump, and release-note draft are committed on `factory/hv-sigill-release`; publication proof remains pending |

## Timeline

All local times are EDT (UTC-04:00). UTC timestamps are included where they
were emitted by CAS.

| Time | Event | Evidence / decision |
| --- | --- | --- |
| 21:49:57 | First retained failed startup | CAS logged a PTY spawn of `codex`; about 252 ms later the boot client reported `Daemon closed connection during initialization`. |
| 21:50:13 | Failure reproduced | A second new factory died during the same initialization phase. |
| 21:52:40 | Failure reproduced again | A third launch showed the same daemon-disconnect pattern. Ordinary retry was ruled out as a remedy. |
| Before 21:57 | Codex itself cleared | Direct Codex startup with the CAS-injected launch arguments worked. `codex doctor` passed runtime, configuration, authentication, and connectivity checks. |
| 21:57:37 | Native process crash captured | `cas codex --new -n codex-strace-repro` dumped core. `coredumpctl info 642419` reported signal 4 (`ILL`) and fault offset `0x2f24543` in `/home/pippenz/.local/bin/cas`. |
| After 21:57 | Illegal instruction identified | Disassembly at the fault offset showed `vcvttsd2usi %xmm0,%rcx`, an AVX-512 instruction. `lscpu` showed AVX/AVX2 but no AVX-512 on the runtime host. |
| After disassembly | Build-path defect isolated | `ghostty_vt_sys/build.rs` was found to omit Zig's `-Dtarget` for native Cargo builds. Zig therefore optimized for the release runner rather than a portable target baseline. |
| About 22:02 | Portable Ghostty archive built | The build script was changed to pass an explicit target for native and cross builds. An opcode scan of the rebuilt `libghostty_vt.a` found neither `vcvttsd2usi` nor `zmm` register use. |
| 22:07:32 | Release rebuild completed | `cargo build --release -p cas --bin cas` completed successfully; the binary size was 53,480,352 bytes. |
| 22:08:03 | Local binary replaced | The rebuilt binary was installed to `/home/pippenz/.local/bin/cas`; source and installed SHA-256 were both `c7ebb290c4b38c83df23a37f032154d3ebcb1fded44f04e68e713ecc3955b3dc`. |
| Final verification | End-to-end recovery proved | A fresh installed-binary launch reached `READY` and `SYSTEM READY`; `cas list --json` reported `is_running: true` and `can_attach: true`. The attached TUI was intentionally ended by a timeout and the test factory was cleaned up. |

## Root cause

### Failure chain

1. `crates/ghostty_vt_sys/build.rs` invokes Zig with
   `-Doptimize=ReleaseFast` to build the statically linked Ghostty VT library.
2. The Zig project uses `b.standardTargetOptions`, so an explicit
   `-Dtarget=...` determines the target CPU baseline.
3. The prior Rust helper deliberately returned `None` when Cargo's target
   matched the build host. Consequently, ordinary native release builds did
   not pass `-Dtarget` to Zig.
4. With no explicit target, Zig was permitted to optimize for the build
   machine's native CPU. The released CAS executable consequently contained an
   unguarded AVX-512 instruction from the Ghostty VT static library.
5. The affected runtime machine supports AVX2 but not AVX-512. During Codex
   `0.147.0` startup, CAS's terminal processing exercised that instruction.
6. The kernel raised `SIGILL`. Because this was a hardware illegal-instruction
   trap rather than a Rust panic, the daemon disappeared without a normal CAS
   error record; the boot client could only report that initialization closed.

The current fix makes supported Cargo targets explicit for both native and
cross builds. The mapping is isolated in
`crates/ghostty_vt_sys/build_support.rs`, called from
`crates/ghostty_vt_sys/build.rs:79`, and covered by
`crates/ghostty_vt_sys/tests/portable_target.rs`.

Supported mappings are:

| Cargo target | Zig target |
| --- | --- |
| `x86_64-unknown-linux-gnu` | `x86_64-linux-gnu` |
| `aarch64-unknown-linux-gnu` | `aarch64-linux-gnu` |
| `x86_64-unknown-linux-musl` | `x86_64-linux-musl` |
| `aarch64-unknown-linux-musl` | `aarch64-linux-musl` |
| `x86_64-apple-darwin` | `x86_64-macos` |
| `aarch64-apple-darwin` | `aarch64-macos` |

Unknown triples now fail closed with an actionable list of supported targets.
Cargo's `TARGET` is required, so neither a missing target nor an unsupported
distributable target can silently fall back to the build host.

### Evidence that establishes the cause

| Observation | Result | What it establishes |
| --- | --- | --- |
| Direct Codex launch under a PTY | Passed | Codex executable, arguments, authentication, and basic terminal use were viable outside CAS. |
| `codex doctor` on `0.147.0` | Runtime/config/auth/connectivity passed | The crash was not explained by a broken Codex install or service access. |
| Repeated `cas codex --new` | Failed during initialization | The defect was deterministic enough to reproduce in the CAS factory path. |
| Core dump, PID 642419 | `Signal: 4 (ILL)` | The CAS process died from an illegal CPU instruction, not a handled application error. |
| Fault address | CAS executable + `0x2f24543` | The failing code was linked into the installed CAS executable. |
| Disassembly at fault | `vcvttsd2usi %xmm0,%rcx` | The faulting operation required AVX-512 encoding. |
| Runtime CPU flags | AVX and AVX2 present; AVX-512 absent | The host could not execute the linked instruction. |
| Prior native-build logic | Returned no Zig target for a matching host/target | It exposed native release artifacts to build-host CPU feature selection. |
| Portable rebuild opcode scan | No `vcvttsd2usi`; no `zmm[0-9]` | The explicit baseline removed the observed AVX-512 signatures from the Ghostty archive. |
| Rebuilt installed CAS factory launch | `READY`, `SYSTEM READY`, running and attachable | The target-pinning change removed the user-visible failure on the same machine and Codex version. |

The correlation with Codex `0.147.0` is a trigger condition, not the root
cause. CAS preflight reported version drift from the validated `0.146.0`
harness, and `0.147.0` output exercised the bad terminal-library path. A
correctly built CAS binary must not contain instructions beyond its supported
runtime baseline regardless of which valid terminal output Codex produces.

### Alternatives ruled out

- **Bad Codex flags:** the same CAS-injected Codex launch shape worked when run
  directly under a PTY.
- **Codex authentication or connectivity:** `codex doctor` passed those checks.
- **A recoverable Rust panic:** the kernel recorded `SIGILL`; no Rust backtrace
  or normal daemon error existed because the process trapped at the CPU level.
- **The runtime machine lacking all vector support:** it has AVX and AVX2; the
  unsupported boundary was AVX-512.
- **Generic CAS state corruption:** rebuilding only the binary with an explicit
  Zig target made a fresh factory start and remain attachable without repairing
  project state.

### Falsification criteria

The diagnosis would need to be reopened if an explicitly targeted rebuild
either still contained the same reachable AVX-512 instruction or reproduced a
`SIGILL` at the same offset on this host. Neither occurred in the local proof.
The broader release-portability claim still needs CI reproduction on official
release infrastructure because the exact release runner CPU was inferred from
the artifact behavior rather than inspected directly.

## Why this was not caught

- Native and cross compilation had different target behavior. Cross builds
  passed `-Dtarget`, while the most common native release path silently selected
  build-host CPU features.
- There was no regression test asserting that a native Cargo target still maps
  to an explicit Zig target. The new tests cover that boundary.
- Release validation exercised the artifact on available build/test machines;
  it did not audit the linked Ghostty archive for instructions above the
  supported x86_64 baseline or run it on a non-AVX-512 machine.
- The validated Codex harness was `0.146.0`, while the installed CLI was
  `0.147.0`. Preflight surfaced this as stale validation but did not block a
  factory, because version drift alone is not proof of incompatibility.
- Direct `codex --version`, direct startup, and `codex doctor` all passed. Those
  checks do not execute CAS's Ghostty-based terminal-processing path.
- `SIGILL` bypassed normal panic/error reporting, so the client saw a secondary
  daemon-disconnect symptom rather than the primary processor fault. A core
  dump was required to expose the signal and fault address.

No individual action caused the outage. The gap was in the build contract and
release portability checks: the distributed artifact did not have an explicit
CPU baseline.

## Corrective actions

| Status | Action | Owner | Verification |
| --- | --- | --- | --- |
| Done locally | Pass a mapped `-Dtarget` to Zig for every supported Cargo target, including native builds. | CAS maintainers | Inspect `build.rs` invocation; build emits the supported mapped target. |
| Done locally | Extract target mapping into testable Rust code and add native/cross/unsupported mapping tests. | CAS maintainers | `cargo test -p ghostty_vt_sys`: 3 passed, 0 failed. |
| Done locally | Rebuild and install a portable CAS binary on the affected host. | Incident operator | `cmp` passed; source and installed SHA-256 match; fresh factory became running and attachable. |
| In progress | Publish the committed `2.54.1` replacement without changing `2.54.0`, then install the published artifact. | CAS maintainers / release engineering | Published checksum; installed `cas --version` identifies the fixed release. |
| Done | Add an x86_64 bundled-Ghostty ISA audit that rejects AVX-512. | Release engineering | Deterministic self-test accepts a baseline fixture, rejects seeded incident-matching `vcvttsd2usi`, fails closed on invalid input, and accepts the built `libghostty_vt.a`. |
| Required before release | Run the released factory binary on baseline x86_64 hardware or a VM with AVX-512 masked. | Release engineering | Current stable Codex reaches `SYSTEM READY`, remains registered, and is attachable. |
| Required before release | Refresh Codex harness conformance for the current supported Codex CLI instead of relying only on the prior validated pin. | Codex harness owner | `cas codex preflight --json` no longer reports the supported installed version as stale. |
| Follow-up | Improve abrupt-daemon-death diagnostics to surface exit signal/core-dump guidance before reporting only “session not running.” | Factory runtime owner | A controlled signal-4 test reports the signal and a diagnostic command without requiring manual strace discovery. |
| Done | Make unknown Cargo targets fail closed instead of falling back to native optimization. | Build/release owners | Policy is encoded in `build_support.rs`; regression test checks the target name and supported-target diagnostic. |

## Provenance

This report was produced from direct inspection and local execution in
`/home/pippenz/Petrastella/cas-src` at source commit
`c2150616fbcabcbff8ece84ed43d5de7dca24847`. The portable build and release-gate
changes are committed at `3d82f3f4`; `2.54.1` release preparation is committed at
`38bc04d6`. It contains no external web-derived claims.

Primary evidence and commands:

```text
cas --version
codex --version
cas doctor
cas codex preflight --json
codex doctor
cas codex --new -n codex-debug-repro
strace -ff ... cas codex --new -n codex-strace-repro
coredumpctl info 642419 --no-pager
objdump -d <installed-cas-at-failure> --start-address=<fault-window> ...
lscpu
cargo test -p ghostty_vt_sys
cargo build --release -p cas --bin cas
objdump -d <rebuilt-libghostty_vt.a> | rg 'vcvttsd2usi|zmm[0-9]'
cmp target/release/cas /home/pippenz/.local/bin/cas
sha256sum target/release/cas /home/pippenz/.local/bin/cas
cas codex --new -n codex-installed-proof-final
cas list --json
git diff --check
```

Fresh verification receipts retained in the incident session:

```text
ghostty_vt_sys tests: 2 passed; 0 failed
portable archive scan: avx512=absent
release build: exit 0
binary comparison: identical
SHA-256 (both): c7ebb290c4b38c83df23a37f032154d3ebcb1fded44f04e68e713ecc3955b3dc
factory proof: READY; SYSTEM READY
registry proof: is_running=true; can_attach=true
attached-launch timeout: exit 124 (expected test bound, not a crash)
```

The core dump remains stored by systemd as recorded by `coredumpctl`; temporary
test factories were cleaned up. An unrelated pre-existing factory was left
running. The exact release-runner CPU and the number of other affected users
remain unknown and are deliberately not inferred as settled facts.
