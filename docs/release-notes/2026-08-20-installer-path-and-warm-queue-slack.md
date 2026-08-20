# 2026-08-20 — Installer PATH wiring + warm merge-queue lanes — Slack draft

Channel: `#cas-internal` (`C0B44GUKDK2`)
Covers main merges: PR #576, #577, #578.

## User thread

**Top-level**

> **Live on production — User**
> Installing Cassy now leaves you with a `cas` command that actually works in a brand-new terminal — and the installer tells the truth about whether it did.

**Reply**

> **Was → Now**
> - *Was:* the one-line installer dropped `cas` into `~/.local/bin` and printed a generic "add this to your PATH" hint. On a fresh Mac (zsh) a new terminal still couldn't find `cas`, and anything launching Cassy without an interactive shell (for example an editor starting the MCP server) missed it entirely. The script said "installed successfully" regardless.
>   *Now:* the installer figures out your real login shell and offers to wire the PATH line into the right file once, idempotently (zsh → `~/.zshenv` so non-interactive launches see it too; bash → `.bashrc` or `.profile`). Say no and it prints the exact line to paste. It then opens a fresh login shell and only reports success when `cas --version` really runs there — otherwise it says plainly that a new terminal can't run `cas` yet and shows the remedy.
> - *Was:* a brand-new machine running bare `cas` with nothing set up dropped you into internal preflight output.
>   *Now:* it prints one friendly line pointing at the command to get started (arrives with the next release build).
> - *Was:* the v3.4.0 notes claimed the `.zshenv` wiring already existed.
>   *Now:* it does — this is the change that makes that sentence true, and the notes were corrected in the meantime.

## Dev thread

**Top-level**

> **Live on production — Dev**
> `cas-install.sh` detects the login shell from `$SHELL`, wires a marker-guarded PATH block into `.zshenv`/`.bashrc`/`.profile` with tty consent or `CAS_WIRE_PATH=1|0`, and verifies in a fresh login shell from the pre-wiring PATH; merge-queue preflight and doctests now run on the trusted runner's persistent target dir (queue run 7m07s, down from 11–14 min cold).

**Reply**

> **Was → Now**
> - *Was:* `scripts/cas-install.sh` only warned "`$INSTALL_DIR` is not in your PATH" and never touched an rc file; the success banner was unconditional. (PR #577)
>   *Now:* `detect_login_shell`/`resolve_rc_file`/`append_path_guard`/`wire_path`: zsh → `${ZDOTDIR:-$HOME}/.zshenv` (interactive-only `.zshrc` misses MCP spawns), bash → `.bashrc` if present else `.profile`, other shells → print the line. The block is idempotent twice over — `# >>> cassy path >>>` markers stop a second append and the block itself is a `case ":$PATH:"` test. Consent opens `/dev/tty` (a real open, not `[ -r ]` — that was ENXIO-spraying under no controlling terminal); no tty + no override = no edit + exact line. `verify_install` runs `$SHELL -lc 'command -v cas'` under `env PATH="$ORIGINAL_PATH"` with `timeout 15`, so the check can't be self-confirmed by the installer's own exported PATH. Bash preamble rejects `| sh`. 8 new seam cases in `scripts/test-cas-install.sh` (fake login shells outside PATH) + 2 macOS cases; `cas-cli/src/cli/first_run.rs` adds `FRONT_DOOR_COMMAND` (`init` today) with a test asserting it resolves to a real clap subcommand. Docs: README, macbook-from-zero, CONTRIBUTING.
> - *Was:* merge-queue runs executed on temporary `gh-readonly-queue/*` refs with no rust-cache on `refs/heads/main`, so hosted preflight (13m27s) and doctests (8m04s) started cold every time; sccache hit rates 0.79% / 5.56%. (PR #576)
>   *Now:* `fast-validation-preflight` and `fast-validation-docs` route through the merge-queue runner selector onto the trusted box (persistent `CARGO_TARGET_DIR`, private sccache) under the same trust boundary as suite-build: `merge_group` only, canonical non-fork repo, `CASSY_MERGE_QUEUE_SELF_HOSTED=enabled`, queue-ref check at execution; hosted remains the fail-safe. `scripts/test-ci-test-tiers.sh` pins both routes; `docs/ci/self-hosted-runner-pilot.md` updated. First measured queue run: 7m07s (suite-build 1m28 → doctests 2m12 → preflight 2m26, serialized on the single runner slot — the remaining gap to <6 min).
> - *Was:* the v3.4.1 fast-publication receipt lived only in the run logs. (PR #578)
>   *Now:* `docs/release-notes/2026-08-20-v3.4.1-slack.md` records the posted announcement, the 166 s tag→publish latency receipt, and the hub-promotion N/A evidence.

## POSTED

Posted 2026-08-20 ~21:33 UTC to `#cas-internal` (`C0B44GUKDK2`):

| Message | Permalink |
|---|---|
| User top-level | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787260790156429 |
| User reply | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787260796916959?thread_ts=1787260790.156429&cid=C0B44GUKDK2 |
| Dev top-level | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787260798321729 |
| Dev reply | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1787260809269979?thread_ts=1787260798.321729&cid=C0B44GUKDK2 |
