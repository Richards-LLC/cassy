# Ben's Cassy setup — complete the Mac setup

Date: 2026-08-18. This is the client-facing completion guide for Ben. It follows
the release-binary path in `2026-08-17-mac-setup-petra-stella.md` and retains
the task-oriented presentation of `ben.html`. The product is **Cassy**; every
command remains `cas`.

## What changed since the clean-install report

- Cloud login now has a reliable path: use the personal token Daniel provides.
  It no longer depends on the broken browser approval page or rate-limit polling.
- Team registration now fails loudly rather than appearing to work silently.
  A clear default team is automatically adopted for a new project.
- The Mac instructions no longer replace the shell halfway through a pasted
  block. They set the current shell PATH and persist it in `~/.zprofile`.
- `cas init` now refuses to scaffold the home directory by accident.
- Cassy 3.0.0 is the product name; its command remains `cas`.

## 1. Basic Mac tools

This guide assumes an Apple Silicon Mac. Install the normal prerequisites:

```zsh
brew install git node
npm install -g @anthropic-ai/claude-code
```

Use `~/.zprofile` for PATH exports because it runs once for a login shell;
`~/.zshrc` runs for every interactive shell. This puts Homebrew and user-local
programs on the PATH now and in new terminals:

```zsh
grep -qxF 'eval "$(/opt/homebrew/bin/brew shellenv)"' ~/.zprofile 2>/dev/null || echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
eval "$(/opt/homebrew/bin/brew shellenv)"
grep -qxF 'export PATH=$HOME/.local/bin:$PATH' ~/.zprofile 2>/dev/null || echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.zprofile
export PATH="$HOME/.local/bin:$PATH"
```

## 2. Install Cassy

```zsh
mkdir -p ~/.local/bin && cd "$(mktemp -d)"
curl -fsSL -o cas.tar.gz https://github.com/Richards-LLC/cassy/releases/latest/download/cas-aarch64-apple-darwin.tar.gz
tar -xzf cas.tar.gz
mv cas ~/.local/bin/cas && chmod +x ~/.local/bin/cas
xattr -d com.apple.quarantine ~/.local/bin/cas 2>/dev/null || true
cas --version
mkdir -p ~/.claude
cas update --user
```

`cas update --user` refreshes Cassy integrations only for AI tool directories
that already exist; it does not install Claude, Codex, or Grok.

## 3. Sign Cassy Cloud in once

Ask Daniel for the personal Cloud API key. Keep it out of repositories,
screenshots, and chat transcripts.

```zsh
cas login --token <YOUR-PERSONAL-API-KEY>
cas whoami
```

This is a once-per-Mac step. Cassy stores it in `~/.cas/cloud.json`; later
projects inherit that login. `.cas/cloud.json` inside a project is only a
refreshed cache when applicable. `cas logout` signs the machine out.

## 4. Set up the real project

Run this inside the repository, never from `~`:

```zsh
mkdir -p ~/code && cd ~/code
git clone <YOUR-REPOSITORY-URL>
cd <repository>
cas init
cas doctor
cas cloud team show
cas cloud status
```

If the team is not visible, have Daniel confirm the Cloud account has been
added. Then run `cas cloud team set <uuid>` or `cas cloud team auto on`. A
project with one resolvable default team is adopted automatically.

## 5. Run Commander hub as a service

The hub is the machine-local service that pairs with
`https://hub.petrastella.io`. It binds to loopback. Tailscale Serve is optional
and exposes it to the Petra Stella tailnet over HTTPS.

```zsh
brew install --cask tailscale
open -a Tailscale
cas hub service install --tailscale-serve
cas hub service status
cas hub pair
```

On macOS, this installs a user-level `launchd` service that starts at login and
survives reboots. Use `cas hub service install` without the flag if tailnet
access is not wanted. In Commander, choose **Pair a machine** and complete the
code flow. `cas hub service uninstall` removes supervision without deleting hub
identity or approved devices.

## 6. Install `cas-update` for source-checkout maintenance

`cas-update` is for a Cassy source checkout. It requires Git, Xcode Command
Line Tools, Rust, and the repository because it builds from source. It is not
required for the release-binary installation above.

```zsh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
source "$HOME/.cargo/env"
cd ~/code/cassy
git submodule update --init --recursive
contrib/shell-helpers/install.sh
cas-update --dry-run
```

The installer copies the canonical helper to `~/.local/bin/cas-update`. Plain
`cas-update` pulls/builds the current source, installs `cas`, migrates and
syncs local Cassy projects, then safely turns over processes that run the old
exact binary.

```zsh
cas-update --sync-only
cas-update --no-restart
```

`--sync-only` migrates/syncs projects without a build or process change.
`--no-restart` builds, installs, migrates, and syncs but leaves runtimes alone.

## 7. Optional `update-ai`

`update-ai` is completely optional and is not Cassy. Daniel provides it from
his `soundwave-config` dotfiles as `~/.local/bin/update-ai`. It refreshes
Claude Code, Grok, Codex, and Cassy in the order and failure policy documented
in its header. Ask Daniel to provide/install it if desired; do not substitute it
for `cas-update` when maintaining a Cassy source checkout.

## API-key locations

| Need | Correct home | Reason |
| --- | --- | --- |
| Cassy Cloud credential | `cas login --token …` writes `~/.cas/cloud.json` | Cassy owns this machine-wide login. |
| Optional AI key supplied through the shell | Export it in `~/.zprofile`, or use the provider's normal login | The login shell provides inherited environment variables. |

Cassy does not parse `~/.zprofile`, `~/.zshrc`, or `~/.zshenv`; code inspection
finds no RC-file reader. It reads its inherited environment. Optional AI
enrichment defaults to `OPENAI_API_KEY`, and semantic-search status recognizes
`VOYAGE_API_KEY`. When a Claude/Codex profile is explicitly selected, Cassy
removes inherited API-key/token variables so an ambient key cannot override the
chosen login.

```zsh
printf '%s\n' 'export OPENAI_API_KEY="<key from Daniel>"' >> ~/.zprofile
source ~/.zprofile
```

## Quick recovery

- `cas` missing: run `source ~/.zprofile` or open a new terminal.
- Commander unavailable: run `cas hub service status`; check Tailscale if used;
  re-pair if prompted.
- Project issue: from the repository run `cas doctor`.
- Cloud issue: repeat `cas login --token …`, then `cas whoami`.

## Verification basis

Checked against the repository on 2026-08-18: the two Mac onboarding guides;
`contrib/shell-helpers/install.sh` and `cas-update`; hub service code; cloud
configuration code; the init guard; Claude/Codex environment handling; and the
local `update-ai` symlink to Daniel's `soundwave-config` dotfiles.
