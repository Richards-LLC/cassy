# Cassy on a Mac from zero — Petra Stella team member guide

Date: 2026-08-17 · Revised 2026-08-20 for the supported macOS release installer and documented team-membership flow · Audience: a new Petra Stella team member with an Apple Silicon Mac and terminal comfort.

**Time: about 15 minutes**, none of it compiling. This uses the published release binary, not a source build. The [MacBook from zero guide](macbook-from-zero.md) has the same binary fast path plus the contributor source-build path.

---

## 1. Prerequisites

This published-binary path supports Apple Silicon (M1–M4) only. Its first
command stops on Intel so an unsupported Mac never reaches the installer.

```bash
if [[ "$(uname -m)" != "arm64" ]]; then
  echo "Unsupported Intel Mac: this Cassy release is Apple Silicon only. Contact the operator."
  exit 1
fi

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

Steps 1 and 2 are safe to paste twice: each shell-config line is appended only
when it is not already present, so a re-run cannot duplicate it.

## 2. Install the Cassy binary

```bash
curl -fsSL https://raw.githubusercontent.com/Richards-LLC/cassy/main/scripts/cas-install.sh | bash
```

Put `~/.local/bin` in `~/.zshenv`, not only `~/.zprofile` or `~/.zshrc`: MCP
subprocesses can start non-interactive shells and must inherit the same PATH.
Apply it for this shell, then confirm the installed release:

```bash
grep -qxF 'export PATH="$HOME/.local/bin:$PATH"' ~/.zshenv 2>/dev/null \
  || echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshenv
export PATH="$HOME/.local/bin:$PATH"
cas --version
```

It also clears the downloaded binary's `com.apple.quarantine` attribute, so no
manual Gatekeeper command is necessary.

## 3. One-time Cassy setup

```bash
mkdir -p ~/.claude   # opt this machine's Claude Code install into Cassy built-ins
cas update --user    # seeds the harness dirs that exist: ~/.claude, ~/.codex, ~/.grok
```

`cas update --user` only writes into a harness directory that already exists — it will not materialize `~/.claude` for someone who does not use Claude Code. On a clean Mac that directory does not exist until Claude Code has run once, which is why the `mkdir -p` is there; without it the command prints `~/.claude does not exist — skipping (Claude Code not installed?)` and seeds only `~/.codex` / `~/.grok` if you happen to have them.

Skip `cas doctor` for now — it reports on a *project*, and you do not have one yet. Step 7 runs it in the right place.

From now on, upgrading is a single command — `cas update` — which fetches the latest release, replaces the binary, then refreshes every local Cassy project (schema, skills, team membership, and cloud-linked sync). To rehearse that sweep without replacing the binary, run `cas update --all-projects --dry-run`.

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

## 5. Cassy Cloud — set and forget

The team runs the self-hosted **Petra Stella Cloud** at **`https://petra-stella-cloud.vercel.app`** — that is the domain you are authenticating against, and it is already the CLI's built-in default, so no endpoint flag is needed. Auth is a personal static API key.

```bash
cas login --token <YOUR-PERSONAL-API-KEY>   # key comes from Daniel; one time
cas whoami                                  # prints the endpoint you are signed in to
```

`cas whoami` exits 0 when you are logged in and non-zero with "Not logged in" when you are not — that exit code is the check. It names your email only when the server supplied one; a token login does not carry an email, so seeing just the endpoint line is normal and correct.

**Use the token, not the browser flow.** `cas login` with no arguments opens the device-approval page at `petra-stella-cloud.vercel.app/device`, and that page currently rejects a valid code with "Missing or invalid Authorization header" even in a signed-in browser. It is a server-side defect, written up in [docs/reports/2026-08-18-cloud-device-login-server-defect.md](../reports/2026-08-18-cloud-device-login-server-defect.md). `cas login --token` never touches that page.

**Once per machine, not once per project.** The credential is stored at user level in `~/.cas/cloud.json`, so this step works from anywhere — including here, before any project exists — and every project you set up afterwards is already signed in. `cas logout` signs the whole machine out.

Team scope and sync status are *project*-scoped, so they are confirmed in step 7, after you have a project. Running them here fails with `Cassy not initialized`.

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

### Adding a new team member

`cas cloud team` selects an existing membership; it cannot invite or grant one.
Before a new teammate starts setup, a current Petra Stella Cloud team
administrator must add the teammate's Cassy Cloud account email to the team.
After that, the teammate runs `cas login --token <their-personal-api-key>` and,
inside an initialized shared project, `cas cloud team show`. If they belong to
multiple teams, they select this one with `cas cloud team set
<team-slug-or-uuid>` and run `cas cloud sync`. If `team show` reports no team,
the administrator should recheck the account email and membership.

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

- `cas` exits immediately with `Killed: 9` and **no Gatekeeper dialog** → the Mach-O signature is invalid or missing. Re-run the installer to replace the binary; if it persists, contact the operator. Do not treat this as a quarantine problem.
- Gatekeeper shows **“cas cannot be opened because the developer cannot be verified”** → clear quarantine: `xattr -d com.apple.quarantine ~/.local/bin/cas`.
- `cas` not found → `~/.local/bin` is missing from PATH; re-check the `~/.zshenv` line, then open a new terminal (or `source ~/.zshenv`).
- `Cassy not initialized` from a cloud command → you are outside a project. `cas login`, `cas logout` and `cas whoami` work anywhere; `cas doctor`, `cas cloud status` and `cas cloud team show` need you to `cd` into a `cas init`-ed project.
- `cas init` refuses with "is your home directory" → that is the guard working; `cd` into the project first.
- Hub unreachable from hub.petrastella.io → `cas hub service status`; confirm Tailscale is connected; re-pair if the Commander shows "Machine needs pairing".
- Anything else → `cas doctor` first, then ask in #cas-internal.

---

Provenance: current release assets include `cas-aarch64-apple-darwin.tar.gz`; the installer and its macOS flow are covered by `scripts/test-cas-install.sh`. Team selection is verified against the `cas cloud team` CLI contract: it resolves cached memberships but does not expose an invite mutation.
