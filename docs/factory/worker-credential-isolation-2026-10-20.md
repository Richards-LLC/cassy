---
title: Worker credential isolation on soundwave
lang: en
---

<main id="main">
<header class="hero">
<p class="eyebrow">Decision brief · soundwave · 20 October 2026 · practitioner · <a class="skip-link" href="#method-and-provenance">Jump to method</a></p>

<h1>HOME-only leaves absolute paths open; bubblewrap closes them.</h1>

<figure>
<figcaption class="figure-title">A mount boundary closes the path HOME-only leaves open</figcaption>

<div class="routes" role="group" aria-label="Comparison of the HOME-only and bubblewrap access paths">
  <section class="route route-fail" aria-labelledby="home-only-title">
    <h2 class="route-title" id="home-only-title"><span>HOME-only</span><span class="status status-fail">FAIL · readable</span></h2>
    <p class="route-path"><code>HOME=/tmp/worker-home</code> → host home stays mounted → absolute canary read succeeds</p>
  </section>
  <section class="route route-pass" aria-labelledby="bwrap-title">
    <h2 class="route-title" id="bwrap-title"><span>Bubblewrap + private HOME</span><span class="status status-pass">PASS · hidden</span></h2>
    <p class="route-path">Private HOME/XDG → operator home is masked → direct, symlink, and proc-root reads fail</p>
  </section>
</div>

<p class="source-line">Soundwave · Linux 7.0 · bubblewrap 0.11.1 · run from v52 c1d815d · synthetic fixture only</p>
</figure>
</header>

## Recommendation

Use a **required bubblewrap filesystem boundary on Linux**, with a fresh per-worker `HOME` and XDG roots inside it. Bind only the assigned worktree, toolchain/cache inputs, and individually named credentials. Keep HOME-only remapping as defense-in-depth, not as the isolation boundary; refuse the spawn when the required namespace is unavailable. Confidence is high that this works for the tested Linux paths and medium that the current tool grants cover a real worker session.

The soundwave prototype blocked the direct, symlink, and `/proc/self/root` routes tested for Neon and Vercel stores. A HOME-only control still read both synthetic canaries. The chosen view also passed a named Rust test, Claude/Codex/GitHub auth-status checks, and an authenticated GitHub HTTPS push dry-run. It did not prove a real branch update, the current SSH remote path, or a complete tool session.

## Decision context

The question is whether factory worker processes can stop seeing the operator's credential stores by filesystem construction while retaining the tools needed to work. The source inspected was the v52 tip `2e589c8e6`; the prototype ran from `c1d815dafb50cc8f88d6d793ccae84ad20bee7ec` on soundwave before that lane advanced. No production code was changed.

| Item | Finding |
| --- | --- |
| Host | soundwave, Linux 7.0.0-38-generic, x86_64, uid 1000 |
| Isolation tool | bubblewrap 0.11.1; unprivileged user and PID namespaces succeeded |
| HOME-only control | Failed: absolute reads of both mock canary files succeeded with a different `HOME` |
| Bubblewrap view | Passed: operator home was masked; direct, symlink, and `/proc/self/root` routes to the mock canaries failed |
| Rust | `cas-core` named test passed 1/1; first isolated compile/test took 26.00 seconds |
| Harness and GitHub auth | Claude, Codex, and `gh` status commands returned 0 using selected read-only auth files |
| Git push path | HTTPS push dry-run to the Cassy remote returned 0 using `gh auth git-credential`; no ref was updated |
| SSH agent | Worker view omitted `SSH_AUTH_SOCK`; `ssh-add -l` returned 2. The host socket had no loaded identity (`ssh-add -l` returned 1). |

## Options

| Option | Absolute-path boundary | Build, auth, and Git evidence | Cost and outcome |
| --- | --- | --- | --- |
| Private `HOME` only | **Fail.** Same-UID code can still open the operator's old absolute paths and follow links to them. | Tools continue to work because the operator's full home remains visible. | Cheapest, but does not meet the requirement. Reject as the security boundary. |
| Bubblewrap + private `HOME` + explicit mounts | **Pass for tested paths.** The host home and its credential roots disappear from the worker mount view. | Rust test 1/1; Claude/Codex/`gh` auth status 0; HTTPS Git push dry-run 0. SSH is unavailable unless deliberately granted. | Recommended Linux prototype. Requires a maintained mount policy and a separate auth/push grant. |
| Separate OS account or VM | Strongest prospective separation; not prototyped here. | Toolchain, worktree ownership, task artifacts, harness login, and GitHub auth all need separate provisioning. | Highest operational cost; useful fallback where namespaces are unavailable or a stronger tenant boundary is required. |

## Why bubblewrap

`HOME=/worker-home` changes default lookups but does not change the filesystem a same-UID process can open. The control test mounted a fixture home at `/tmp/operator-home`, set `HOME=/tmp/worker-home`, then read both fixture files by absolute path. That disproves HOME-only isolation without touching real credential contents.

The Linux prototype created a user and PID namespace, a private `/tmp`, fresh `/proc` and `/dev`, a private `HOME`/XDG tree, and a writable bind for the assigned worktree. It made the host home unavailable and used read-only binds for selected auth files and Rust toolchain/cache paths. It used synthetic Neon/Vercel canaries for read attempts; it never opened actual Neon or Vercel credential contents. The direct path, a private-home symlink to the old path, and `/proc/self/root` all failed to reach the canaries.

This prototype used a read-only bind of `/` before masking the host home. It is evidence for the tested home paths, not a finished least-privilege root. Production should construct an allowlisted root view and explicitly exclude other host mount points, inherited file descriptors, and unrelated worktrees. The assigned worktree remains a trust boundary: committed secrets or a `.vercel` directory inside that worktree would still be visible.

## Tool continuity and limits

- **Rust and Cargo:** read-only binds of `.cargo/bin`, `.cargo/registry`, `.cargo/git`, `.rustup`, and the agent-build cargo shim were sufficient for the named test. No global Cargo config or whole `.cargo` directory was exposed. The worktree target directory remained writable. Shared `sccache`, Zig, and other build inputs need separate mounts if the production worker recipe uses them.
- **Claude and Codex:** `claude auth status` and `codex login status` both returned 0 after mounting only their auth files read-only into the private home and binding their executable trees. This proves local auth discovery, not a complete interactive launch. Trust state, config, MCP settings, caches, and any files the harness writes need a private writable location.
- **GitHub:** `gh auth status` returned 0 with only the host entry file mounted read-only. An HTTPS `git push --dry-run` using `gh auth git-credential` returned 0. The token scope was not inspected. Prefer a dedicated, short-lived, repository-scoped write token or a credential broker; do not pass a personal operator token as the production grant.
- **Git remote:** the checkout's normal `origin` is SSH. The successful test used an HTTPS URL with the explicit GitHub credential helper. With the SSH socket omitted, the existing SSH remote would not authenticate. Decide whether worker Git is rewritten to HTTPS or whether a scoped SSH signing capability is explicitly granted.
- **SSH agent:** `SSH_AUTH_SOCK` was absent from the sandbox environment, and `ssh-add -l` failed as expected. The host socket reported no identities, so this run could not establish that an SSH key would work through a deliberate socket grant. Default-deny is the safer setting; a forwarded agent remains a signing capability even when private key bytes never leave the host.
- **Namespace availability:** soundwave supports the tested unprivileged namespaces. That is a host measurement, not a fleet guarantee. Startup must probe the exact features and fail closed with a clear remedy when they are unavailable.

## macOS and prowl costs

Bubblewrap is Linux-specific. This prototype did not run on prowl/macOS. The `sandbox-exec` CLI is deprecated and described as no longer supported in the [Apple Developer Forums discussion](https://developer.apple.com/forums/thread/661939); do not label it an equivalent backend without a tested policy on supported macOS versions. A macOS backend must separately validate filesystem denial, keychain behavior, inherited sockets, worktree access, toolchains, harness auth, and Git push. If that cannot be proven, strict isolation should refuse worker launch on macOS until a separate OS account or VM path is available.

The migration cost is mostly duplicated environment plumbing: private home and XDG state, Rust/Cargo caches and toolchains, harness auth/trust, the GitHub credential path, and any per-worker scratch or artifact roots. Shared checkouts also need compatible ownership and write access. A separate account or VM adds provisioning and cleanup for each of those resources.

## Implementation breakdown

1. Define a trusted, typed `WorkerLaunchPlan` from host policy and resolved worker context; task text must not select mount paths or credential grants.
2. Add a Linux bubblewrap adapter that creates a private writable home, XDG roots, proc/dev namespaces, an allowlisted read-only system root, and an explicit writable checkout. Probe capabilities before accepting work; fail closed.
3. Pass the plan through the common `Pty::spawn` boundary while retaining the intended harness command in audit and telemetry. Current worker configuration is assembled in `Mux::build_add_worker_config` (`crates/cas-mux/src/mux.rs:863`), reaches `Pty::spawn` through `Pane::worker` (`crates/cas-mux/src/pane/mod.rs:416`), and starts the direct command while inheriting daemon environment (`crates/cas-pty/src/pty.rs:2504`). Codex currently pins workers to host `HOME` (`crates/cas-pty/src/pty.rs:928`); the existing environment credential policy removes selected variables but does not hide files (`crates/cas-pty/src/pty.rs:1147`).
4. Model named grants for harness auth, Rust toolchain/cache, GitHub write access, and SSH. Keep SSH disabled by default; use per-worker auth or a broker instead of mounting broad operator credential stores.
5. Add one production-path deletion regression with synthetic Neon/Vercel canaries. It should prove direct, symlink, proc-root, and inherited-descriptor attempts fail while the selected worktree, named Cargo test, harness auth, and Git push path succeed. Run it on a Linux host with namespaces enabled and one where they are disabled.
6. Keep macOS in a separate follow-up until its sandbox contract and cost model are demonstrated; otherwise provide an explicit unsupported/blocked result rather than falling back to HOME-only behavior.

## What would falsify this recommendation

The recommendation fails if a canary placed at a known Neon/Vercel credential path can be read from any worker-visible path, including symlinks, `/proc`, an inherited descriptor, a host socket, or an accidentally exposed worktree. It also fails operationally if a normal named Cargo test, selected harness auth, or the approved Git push path cannot run through the exact worker launch plan. A bwrap binary being installed is not enough: the specific namespace and mount operations must succeed.

## Method and provenance

The measurements below are from 2026-10-20 on soundwave. The HOME-only control and bubblewrap test used only synthetic fixture file contents (`MOCK_NEON_CANARY` and `MOCK_VERCEL_CANARY`); actual Neon/Vercel credential contents were not read. Auth status commands suppressed stdout/stderr and recorded exit status only. Git used `--dry-run`; no remote ref was changed.

Key commands and checks:

```text
uname -a
bwrap --version
bwrap --unshare-user --unshare-pid --die-with-parent --ro-bind / / --tmpfs /tmp --dev /dev --proc /proc ... --tmpfs /home/pippenz --clearenv --setenv HOME /tmp/worker-home ...
cargo test -p cas-core --lib a_scope_sets_unsets_and_falls_through_then_restores
claude auth status
codex login status
gh auth status
git -c credential.helper="!gh auth git-credential" -c remote.probe.url=https://github.com/Richards-LLC/cassy.git push --dry-run probe HEAD:refs/heads/cas-b47c-isolation-probe
ssh-add -l
```

The ellipsis in the bwrap command denotes the explicit fixture, toolchain, checkout, and selected auth-file binds described above; the exact invocation was a one-off experiment, not a production wrapper. Code-path references were checked at `2e589c8e6`. Bubblewrap's upstream description of user and mount namespaces is in the [bubblewrap README](https://github.com/containers/bubblewrap/blob/main/README.md). The prototype result is local evidence, not a fleet security certification.

</main>
<footer>Source: <a href="worker-credential-isolation-2026-10-20.md">Markdown report</a> · prototype commit <code>c1d815daf</code> · analysis date 20 October 2026.</footer>
