# CAS on a Mac from zero — Petra Stella team member guide

Date: 2026-08-17 · Revised 2026-08-18 after a clean-install run found the step order, `cas cloud team`, and `cas update --user` claims wrong · Verified against: CAS v2.72.0 (released 2026-08-17), `cas` CLI help and measured command behaviour on a live install · Audience: a new Petra Stella team member with an Apple Silicon Mac and terminal comfort.

**Time: about 15 minutes**, none of it compiling. This uses the published release binary, not a source build. (The older `docs/onboarding/macbook-from-zero.md` is the source-build path for people hacking on CAS itself; it predates current releases — see Known gaps.)

---

## 1. Prerequisites

Apple Silicon (M1–M4). Check: `uname -m` → `arm64`.

```bash
# Homebrew (skip if `brew --version` works)
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
grep -qxF 'eval "$(/opt/homebrew/bin/brew shellenv)"' ~/.zprofile 2>/dev/null \
  || echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
eval "$(/opt/homebrew/bin/brew shellenv)"

# Git + Node, then Claude Code
brew install git node
npm install -g @anthropic-ai/claude-code
```

Claude Code ≥ 2.1.126 is required (earlier 2.1.117–2.1.125 had a factory-mode rendering crash); a fresh install today is far past that.

Steps 1 and 2 are safe to paste twice: each `~/.zprofile` line is appended only when it is not already present, so a re-run cannot duplicate it.

## 2. Install the CAS binary

> **Heads-up (gap #469):** the documented one-liner installer (`cas-install.sh`) currently refuses macOS even though macOS builds ship with every release. Until that fix lands, download the release asset directly:

```bash
mkdir -p ~/.local/bin && cd "$(mktemp -d)"
curl -fsSL -o cas.tar.gz \
  https://github.com/Richards-LLC/cassy/releases/latest/download/cas-aarch64-apple-darwin.tar.gz
tar -xzf cas.tar.gz
mv cas ~/.local/bin/cas && chmod +x ~/.local/bin/cas
xattr -d com.apple.quarantine ~/.local/bin/cas 2>/dev/null || true  # Gatekeeper
grep -qxF 'export PATH=$HOME/.local/bin:$PATH' ~/.zprofile 2>/dev/null \
  || echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.zprofile
export PATH=$HOME/.local/bin:$PATH   # this shell; ~/.zprofile covers new ones
cas --version                        # expect: cas 2.72.0 (or newer)
```

The block used to end with `exec zsh -l`, which replaced the shell mid-paste so the lines after it never ran. `export PATH=…` does the same job for the current shell and lets the rest of the block finish. New terminals pick the PATH up from `~/.zprofile`.

## 3. One-time CAS setup

```bash
mkdir -p ~/.claude   # opt this machine's Claude Code install into CAS built-ins
cas update --user    # seeds the harness dirs that exist: ~/.claude, ~/.codex, ~/.grok
```

`cas update --user` only writes into a harness directory that already exists — it will not materialize `~/.claude` for someone who does not use Claude Code. On a clean Mac that directory does not exist until Claude Code has run once, which is why the `mkdir -p` is there; without it the command prints `~/.claude does not exist — skipping (Claude Code not installed?)` and seeds only `~/.codex` / `~/.grok` if you happen to have them.

Skip `cas doctor` for now — it reports on a *project*, and you do not have one yet. Step 7 runs it in the right place.

From now on, upgrading is a single command — `cas update` — which fetches the latest release, replaces the binary, and runs schema migrations. Run it when release announcements land in #cas-internal; nothing else to maintain.

## 4. Hub service (Commander access)

The hub is the machine-local service the web Commander ([hub.petrastella.io](https://hub.petrastella.io)) talks to. It should run as a boot-persistent service, published tailnet-only over HTTPS.

```bash
brew install --cask tailscale   # skip if Tailscale is already on the Mac
open -a Tailscale               # log in to the tailnet once

cas hub service install --tailscale-serve   # user-level launchd service, survives reboots
cas hub service status                      # supervision + hub health in one view
```

### Pair your browser (once per device)

```bash
cas hub pair        # prints a 10-minute one-time pairing invitation
```

Or start from the phone/browser: open hub.petrastella.io → **Pair a machine**, then approve the short code on the Mac with `cas hub authorize`. Manage devices later with `cas hub auth`.

Pairings expire (90 days absolute, 30 days idle). Since v2.72.0 an expired pairing shows **"Machine needs pairing"** with a **Re-pair** button in the Commander — it is not offline, just re-pair and continue.

## 5. CAS Cloud — set and forget

The team runs the self-hosted **Petra Stella Cloud** at **`https://petra-stella-cloud.vercel.app`** — that is the domain you are authenticating against, and it is already the CLI's built-in default, so no endpoint flag is needed. Auth is a personal static API key.

```bash
cas login --token <YOUR-PERSONAL-API-KEY>   # key comes from Daniel; one time
cas whoami                                  # prints the endpoint you are signed in to
```

`cas whoami` exits 0 when you are logged in and non-zero with "Not logged in" when you are not — that exit code is the check. It names your email only when the server supplied one; a token login does not carry an email, so seeing just the endpoint line is normal and correct.

**Use the token, not the browser flow.** `cas login` with no arguments opens the device-approval page at `petra-stella-cloud.vercel.app/device`, and that page currently rejects a valid code with "Missing or invalid Authorization header" even in a signed-in browser. It is a server-side defect, written up in [docs/reports/2026-08-18-cloud-device-login-server-defect.md](../reports/2026-08-18-cloud-device-login-server-defect.md). `cas login --token` never touches that page.

**Once per machine, not once per project.** The credential is stored at user level in `~/.cas/cloud.json`, so this step works from anywhere — including here, before any project exists — and every project you set up afterwards is already signed in. `cas logout` signs the whole machine out.

Team scope and sync status are *project*-scoped, so they are confirmed in step 7, after you have a project. Running them here fails with `CAS not initialized`.

**When to sync: normally never by hand.** Auto-sync is on by default (`cloud.auto_sync = true`) and runs every 60 seconds while you are logged in. Manual commands exist for exactly three situations:

| Situation | Command |
| --- | --- |
| You just joined and want the team's existing memories for a project now | `cas cloud team-memories --full` |
| You are about to close the laptop after a burst of work and want it pushed *now* | `cas cloud sync` |
| Something looks stale and you want to see why | `cas cloud status`, then `cas cloud queue` |

Since v2.72.0 sync is also honest: it prints a summary of any task-status changes it applied, never silently reopens closed work, and parks permanently-rejected pushes once with one concise reason instead of retrying forever.

## 6. How team memory sharing works

- **In a team-linked project, new memories auto-share to the team.** When you (or your agents) store a learning/preference/context note, it is promoted to the team pool by default — teammates' sessions can recall it.
- **Keeping something personal is a per-memory choice** (the `personal` flag when storing). Personal notes stay on your account only.
- **Pre-team history can be backfilled:** `cas memory share` retroactively shares personal memories you created before joining the team; `cas memory unshare <id>` flips any shared memory back to private.
- **Pulling the team pool:** happens automatically with sync; `cas cloud team-memories` forces an immediate incremental pull for the current project (`--full` re-pulls everything, `--dry-run` previews).
- Memories are project-scoped by the project's canonical slug, which auto-derives from the git remote — so `cas init` in a shared repo is all it takes for your context to line up with the team's.

## 7. Per project

Run this from inside the project, never from your home directory — `cas init` scaffolds `.cas/`, `CLAUDE.md`, `.gitignore`, `.mcp.json` and `scripts/` into whatever directory you are standing in. (After v2.72.0 it refuses to do that in `$HOME` and tells you to `cd` first; on 2.72.0 itself it does it silently.)

```bash
cd ~/code/some-project
cas init              # tasks/memories/knowledge wire up, cloud slug derives
cas doctor            # green across the board for this project
cas cloud team show   # confirm the Petra Stella team scope (bare `cas cloud team` only prints help)
cas cloud status      # confirm: logged in, team set, auto-sync on
```

If `cas cloud team show` reports no team, set it once for this project with `cas cloud team set <uuid>` or turn on inheritance of your user-level default with `cas cloud team auto on`.

Then start working: bare `cas` launches the factory TUI with defaults.

## Troubleshooting

- Binary killed instantly on first run → Gatekeeper: `xattr -d com.apple.quarantine ~/.local/bin/cas`.
- `cas` not found → `~/.local/bin` missing from PATH; re-check the `~/.zprofile` line, then open a new terminal (or `source ~/.zprofile`).
- `CAS not initialized` from a cloud command → you are outside a project. `cas login`, `cas logout` and `cas whoami` work anywhere; `cas doctor`, `cas cloud status` and `cas cloud team show` need you to `cd` into a `cas init`-ed project.
- `cas init` refuses with "is your home directory" → that is the guard working; `cd` into the project first.
- Hub unreachable from hub.petrastella.io → `cas hub service status`; confirm Tailscale is connected; re-pair if the Commander shows "Machine needs pairing".
- Anything else → `cas doctor` first, then ask in #cas-internal.

## Known gaps (filed 2026-08-17)

- **[#469](https://github.com/Richards-LLC/cassy/issues/469)** `cas-install.sh` rejects macOS — the reason this guide hand-downloads the release asset. When fixed, step 2 collapses to one curl|bash line.
- **[#470](https://github.com/Richards-LLC/cassy/issues/470)** the older from-zero doc is source-build-only with stale claims about release channels; this guide supersedes it for non-contributors.
- **[#471](https://github.com/Richards-LLC/cassy/issues/471)** there is no CLI or documented flow for the *admin* side of team invites — a Petra Stella admin must invite your account in the CAS Cloud web UI before `cas cloud team show` will show the team.

---

Provenance: commands and flags verified against `cas 2.71.0/2.72.0` `--help` output and `cas config` defaults on the machine `soundwave-linux` (2026-08-17); release asset names and checksums from the published v2.72.0 GitHub release; memory-sharing semantics from the CAS memory MCP contract and `cas memory`/`cas cloud team-memories` CLI. The 2026-08-18 revision additionally measured each step's real behaviour outside and inside a project (`cas login --token`, `cas whoami`, `cas cloud team show`, `cas cloud status`, `cas doctor`, `cas init`) and read `sync_user_builtins` in `cas-cli/src/cli/update.rs` for the `--user` claim.
