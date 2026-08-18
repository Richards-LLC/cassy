# Cassy shell helpers — how they're made and what to consider for distribution

**Date:** 2026-04-30
**Status:** ideation / pre-decision
**Trigger:** distribution planning — the daily-driver `cas-login`, `cas-update`, `cas-refresh` helpers are author-machine-specific and need a portability story before any user other than the maintainer can rely on them.

## TL;DR

There are three "aliases" in `~/.local/bin/` (they're full bash scripts, not shell aliases — calling them aliases is shorthand). They wrap gaps in the `cas` binary itself: multi-project sweeps, build-from-source, env-driven auth, and daily-use orchestration. Each is hand-rolled, lives only on the author's machine, and has hard-coded assumptions baked in. Shipping them to other users means picking one of three strategies (bake into binary / ship as separate package / document as dotfiles) and patching half a dozen portability blockers either way.

## Inventory

### `~/.local/bin/cas-login` (~1.6 KB)
Env-driven wrapper around `cas auth login`. Reads `CAS_CLOUD_TOKEN` and `CAS_CLOUD_ENDPOINT` from the shell environment (set by a `# >>> Cassy Cloud auth <<<` block in `~/.bashrc`), then `exec cas auth login --token "$CAS_CLOUD_TOKEN" --endpoint "$CAS_CLOUD_ENDPOINT"`. Also surfaces `--whoami` and `--logout` as passthroughs.

**Why it exists:** `cas auth login` assumes interactive terminal. `cas-login` makes "log in with the token I already have in env" a one-word command and surfaces a useful error if the env vars are missing.

### `~/.local/bin/cas-update` (~11.6 KB after today's fix)
Build-from-source + multi-project sync orchestrator. Steps:
1. `git pull` cas-src, `cargo build --profile release-fast`, install to `$HOME/.local/bin/cas`.
2. Find every `<project>/.cas/cas.db` under `$HOME` (with prune list for noise dirs).
3. For each project: `cas update --schema-only` then `cas update --sync`, with a one-line summary per project.

Flags: `--build-only`, `--sync-only`, `--no-pull`, `--debug`, `--dry-run`, `--branch`, `--projects`. Env overrides: `CAS_SRC`, `CAS_INSTALL`, `CAS_PROJECT_ROOTS`.

**Why it exists:** `cas update` is per-project. After a binary upgrade, schema migrations have to land on every project's DB before that project's next session, or the binary crashes. The wrapper is the only way today to do that across all 35 projects without a manual loop.

### `~/.local/bin/cas-refresh` (~9.9 KB)
Daily-use orchestrator. Calls `cas-update --build-only`, then sweeps projects (using its own discovery + exclusion list, *not* delegated to `cas-update --sync-only`), then `cas-login`, then per-project `cas cloud sync`, then `exec cas factory`.

Flags: `--no-update`, `--no-login`, `--no-sync`, `--no-factory`, `--verbose`. Hard-coded exclusion lists at lines 64-66: `EXCLUDE_FRAGMENTS='Penguinz|cas-src|petra_stella_tools'`, `EXCLUDE_HOME_DIRS='ai|ai-toolkit|Apps'`.

**Why it exists:** the maintainer wants one command to "freshen everything and start working." The exclusion logic exists because cloud-sync should NOT run on certain projects (different cloud endpoints, sensitive workspaces, infra-only repos).

## Anatomy — what these are made of

All three are **plain bash scripts**, not shell aliases:

- Shebang: `#!/usr/bin/env bash`
- Strictness: `set -euo pipefail` at the top of every file
- Help: `sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \?//'` — extracts the leading comment block as the `--help` payload.
- Color/no-TTY guard: `if [ -t 1 ]; then BOLD=$'\e[1m'; ...; fi`
- Output helpers: `info()`, `warn()`, `error()`, `step()`, `dim()` — printf-based, no external deps.
- Project discovery: `find $HOME -prune-noise -path '*/.cas/cas.db'` (cas-update inlines this; cas-refresh extracts it as `discover_all_projects()`).

There is **no shared library**. cas-refresh re-implements project discovery and adds an `apply_exclusions` function on top — both scripts duplicate the same `parse_local_summary` regex pipeline with a known bug (see below).

## Why "aliases" is the wrong word

Shell aliases live in `~/.bashrc` as `alias name='command'` and are evaluated at line-read time inside the calling shell. These are **standalone executables in `$PATH`**:

| Property | shell alias | these scripts |
|---|---|---|
| Distribution | source-and-rebase rc-file changes | drop a file in `$PATH` |
| Argument parsing | shell expansion only | full `getopts`-style parsing |
| Subshell safety | runs in current shell | always runs in subshell |
| Crontab/non-interactive | no | yes |

The implication for distribution: shipping a "shell alias" means modifying every user's rc file (per shell — bash, zsh, fish all differ). Shipping a script means dropping a file. The latter is cleaner and is what we already do; the doc just calls them aliases by habit.

## Hard-coded assumptions (the portability blockers)

If we hand any of these to a second user as-is, they break on first run. Concrete blockers:

1. **Path: `$CAS_SRC` defaults to `$HOME/Petrastella/cas-src`** — env override exists, but no other user has that path. Acceptable only if `CAS_SRC` is documented as a required env var.
2. **Path: `$CAS_INSTALL` defaults to `$HOME/.local/bin/cas`** — same story. Could honor `cargo install` conventions or `XDG_BIN_HOME`.
3. **Build profile: `release-fast`** — assumes the cas-src `Cargo.toml` defines that profile. True today, but a second user installing from a tarball or different fork might not have it.
4. **Rust toolchain assumption** — `cas-update` runs `cargo build`. Distributing this means the user has Rust. Most distributions of `cas` are *binary* releases. Build-from-source isn't usable for a non-developer audience.
5. **Auth env block in `~/.bashrc`** — `cas-login` requires `CAS_CLOUD_TOKEN` + `CAS_CLOUD_ENDPOINT` exported. Comment in the script says "set by the `# >>> Cassy Cloud auth <<<` block in `~/.bashrc`" — that block is hand-edited and never automated. Bootstrapping a new user requires manual rc-file surgery.
6. **Bash-only shebang** — `#!/usr/bin/env bash`. Fine on macOS/Linux. Windows users running git-bash also work. Native Windows or fish-only users do not.
7. **cas-refresh exclusion lists** are hand-coded in the script body (`Penguinz|cas-src|petra_stella_tools`, `ai|ai-toolkit|Apps`). These are author-specific. A shipped version needs either a config file (`~/.cas/refresh.toml` excludes), a discovery convention (per-project `.cas/cloud-sync = false`), or both.
8. **Output-parsing fragility** — both `cas-update` and `cas-refresh` parse the cas binary's `--sync` and `--schema-only` stdout with regexes (`Schema (up to date|migrated to) \(v[0-9]+\)`, `Synced [0-9]+ skills, removed [0-9]+`). When the binary's wording changed in any past release, the wrappers silently produced empty summaries. There is no contract between the binary and the wrappers; this is a pure scrape.
9. **The `set -e` + `pipefail` + bare-assignment-grep silent-exit trap** — see next section. Half-fixed today.

## The pipeline-grep silent-exit bug (carry-over from today)

Today (2026-04-30) `cas-update` was silently exiting at iter 1 of the project sweep. Root cause:

```bash
schema=$(printf '%s' "$combined" | grep -oE 'Schema (up to date|migrated to) \(v[0-9]+\)' | head -1 \
         | sed -E 's/Schema (up to date|migrated to) \((v[0-9]+)\)/\2/')
```

Under `set -euo pipefail`, when grep finds no match (e.g., a "warm" project with no skill diff and no schema migration line in its `cas update --sync` output), `grep` exits 1, `pipefail` propagates that, the plain assignment returns 1, and `set -e` exits the whole script with no error message. The user just sees the script vanish.

Fixed in `cas-update` today by appending `|| true` to each pipeline. **`cas-refresh` still contains the same vulnerable pattern** at lines 110-115 (`parse_local_summary`). Symptoms there are milder because the function is invoked inside `$(...)` — the subshell exits silently, producing an empty summary string, but the parent script keeps going. The "ok" line just looks degraded. Worth patching the same way before distribution.

## Distribution options

### A. Bake the orchestration into the `cas` binary
Add `cas update --all`, `cas auth login --env`, `cas factory --refresh-first` (or similar) as proper subcommands. The shell helpers become 3-line wrappers — or go away entirely.

- **Pro:** one binary, no shell-script portability problem, output parsing becomes structured (the binary owns both ends), exclusion config can live in `cas.toml`.
- **Pro:** removes the binary-vs-wrapper output-format coupling that already broke once today.
- **Con:** scope creep into the cas crate. Multi-project orchestration is a different domain than per-project cas operations.
- **Con:** "build cas from source" can't go in the binary itself (chicken-egg), so `cas-update`'s build half stays a script.
- **Cost:** medium — a few new subcommands plus a project-discovery module. Mostly mechanical.

### B. Ship as a separate package
A `cas-tools` or `cas-cli-helpers` repo (cargo / npm / homebrew formula). Installer drops the scripts in `$PATH`, optionally writes a starter `~/.cas/refresh.toml` exclusion file.

- **Pro:** keeps cas binary focused. Iteration speed for the helpers is decoupled from the cas release cycle.
- **Pro:** users who don't want orchestration can skip it.
- **Con:** two release pipelines to keep in lockstep — output-format coupling problem persists, only with one more layer of indirection.
- **Cost:** low to set up, medium to maintain — every cas release potentially breaks the helpers.

### C. Document as user-installable dotfiles
Treat the scripts as reference implementations. Publish them in the cas-src repo under `contrib/shell-helpers/` with a README that says "copy these to your `$PATH`, set these env vars, edit these exclusion lists."

- **Pro:** zero ongoing distribution work.
- **Pro:** users see the moving parts and can adapt to their own environment.
- **Con:** every user does the same setup work. No skill transfer; bugs found by user N are not auto-fixed for user N+1.
- **Cost:** trivial.

### Recommendation

A is the right long-term answer for `cas update --all` and `cas auth login --env` — those are core cas operations dressed in a shell wrapper, and absorbing them into the binary removes the output-parsing coupling entirely. C is fine for `cas-refresh` for now: it's intrinsically opinionated (which projects to sync, which to exclude, what "freshen everything" means for *your* workflow), and trying to make it user-configurable is a larger design problem than today's audience justifies.

So: bake the multi-project sweep + env-auth into the binary; leave the orchestrator-with-exclusions as a documented dotfile in `contrib/`.

## Action items if we decide to ship

In rough priority order:

1. **Patch `cas-refresh`'s `parse_local_summary`** the same way `cas-update` was patched today (`|| true` after each grep pipeline, or `local var=$(... || true)`). Pure carry-over fix.
2. **Decide on `cas update --all`** — discovery + iteration + summary, equivalent to `cas-update --sync-only` minus the build step. If yes: design the project discovery API (env vs config vs scan) and the exclusion model.
3. **Decide on `cas auth login --env`** — read `CAS_CLOUD_TOKEN` / `CAS_CLOUD_ENDPOINT` directly inside the binary, with the same "missing var → friendly error" UX `cas-login` provides.
4. **Stabilize the cas binary's `--sync` / `--schema-only` output format** under a documented contract, so any future helpers (in or out of tree) can parse without scraping. Even if the wrappers get absorbed, a structured output mode (`--json`?) is cheap insurance.
5. **Move helpers to `contrib/shell-helpers/` in the repo** with their own README, and update the existing `project_cas_local_wrappers.md` memory to point there instead of the bare `~/.local/bin/` location.
6. **Generalize hard-coded paths and exclusion lists** in any version that ships, even if just to a TOML config file in `~/.cas/`.

## Open questions

- Is there appetite for a `--json` output mode on `cas update`? It would let the multi-project sweep parse structured data instead of regex-scraping stdout.
- Do we want to support shell completion (bash + zsh) for whatever ships? That's a separate distribution surface.
- Where does `cas-refresh`'s "launch factory at the end" step belong if we absorb the rest? `cas factory --refresh-first` is one option; `cas refresh && cas factory` is the other and probably better.
- The maintainer's `~/.bashrc` "Cassy Cloud auth" block isn't checked in anywhere. Should the cas binary emit it via `cas auth login --print-shell-block` for users to drop into their rc?

## Related

- Memory: `project_cas_local_wrappers.md` — current pin: "they're in `~/.local/bin/`, not cas-src".
- Memory: `feedback_set_e_pipefail_assignment_grep.md` — today's gotcha; carry-over to cas-refresh pending.
- Existing ideation: `2026-04-28-cas-harness-portability-ideation.md` (untracked, referenced for context only).
