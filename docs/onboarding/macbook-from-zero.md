# MacBook from zero to CAS

End-to-end setup for installing CAS on a Mac **from `Richards-LLC/cassy`**, built from source. Assumes you are comfortable with a terminal but have never installed CAS or Claude Code before.

> **What this is not.** This guide does not use the legacy public `install.sh` or Homebrew paths, which distributed **v1.0** (2026-03-12) and **v0.2.1** respectively — months behind the development tip at the time. It intentionally builds from the current source repository instead.

> **Hard requirement:** Apple Silicon Mac (M1 / M2 / M3 / M4). The build itself works on Intel Mac, but the vendored Ghostty terminal renderer and several profiling paths are tested only on `aarch64-apple-darwin`. If you're on Intel, expect to debug.

---

## What you'll end up with

- `cas` binary built from your fork at `~/.local/bin/cas`
- Claude Code installed and configured to talk to `cas` over MCP
- A project initialized with `cas init`
- The CAS factory TUI launching successfully
- A one-liner upgrade workflow (`git pull && cargo build && install …`)

Total time: **~30–45 minutes** end-to-end on a fresh machine (first `cargo build` is the long pole at ~5–15 minutes).

---

## Step 1 — Confirm your Mac

```bash
# Should print "arm64". x86_64 means Intel Mac (proceed anyway, but expect rough edges).
uname -m

# macOS Sonoma (14) is the supported baseline. Older versions may work but are untested.
sw_vers
```

---

## Step 2 — Install Homebrew (if you don't have it)

```bash
# Skip if `brew --version` already prints something.
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

After install, **add Homebrew to your shell** — the installer prints these two lines but they are easy to miss, and without them `brew` (and everything you install) won't be on your `$PATH`:

```zsh
echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
eval "$(/opt/homebrew/bin/brew shellenv)"
```

Then verify:

```bash
brew doctor   # expect: "Your system is ready to brew."
which brew    # expect: /opt/homebrew/bin/brew
```

> **Why `~/.zprofile` and not `~/.zshrc`:** `~/.zprofile` runs once per login, which is the right place for `$PATH` exports. `~/.zshrc` runs every interactive shell — putting `$PATH` setup there works but adds startup overhead. zsh has been the default macOS shell since Catalina (10.15), so this is what you want by default.

---

## Step 3 — Install Xcode Command Line Tools

Rust on Mac needs Apple's clang + linker + SDK headers. These come from the Command Line Tools package, **not** the full Xcode app.

```bash
# Skip if `xcode-select -p` already prints a path.
xcode-select --install
```

A GUI popup appears; click **Install** and wait ~5–10 minutes. Verify:

```bash
xcode-select -p           # expect: /Library/Developer/CommandLineTools (or /Applications/Xcode.app/...)
clang --version           # expect: Apple clang version ...
```

> If `cargo build` later errors with `linker 'cc' not found` or `ld: library not found for -lSystem`, this step was skipped or incomplete. Re-run `xcode-select --install`.

---

## Step 4 — Install Git, Node, and Zig

```bash
brew install git node zig
```

- **git** — clone the fork and manage the `vendor/ghostty` submodule.
- **node** — Claude Code is an npm package.
- **zig** — `crates/ghostty_vt_sys/build.rs` links libghostty-vt by invoking the Zig compiler. The build script searches `$ZIG` → `which zig` → `.context/zig/zig`, so Homebrew's `zig` is found automatically. The repo also ships `./scripts/bootstrap-zig.sh` which downloads Zig **0.15.2** into `.context/zig/` if you'd rather not use Homebrew's Zig; either works.

Verify:

```bash
git --version
node --version
zig version    # expect: 0.14.x or 0.15.x — both work
```

> **Zig version drift:** `cas-cli/mise.toml` pins `zig = "0.14.1"` while `scripts/bootstrap-zig.sh` downloads `0.15.2`. Both build cleanly today; this is a known minor inconsistency, not a problem to chase.

---

## Step 5 — Install Claude Code

```bash
npm install -g @anthropic-ai/claude-code
```

> **No pin required as of 2026-05-04.** Versions 2.1.117 – 2.1.125 shipped a React-Ink rendering regression that crashed Claude Code with `<Box> can't be nested inside <Text>` in agent-teams (factory) mode. **Anthropic fixed it in 2.1.126**, and CAS v2.14.0 integrates upstream changelog entries through **2.1.139** (`CLAUDE_PROJECT_DIR` for stdio MCP, exec-form hook args, `skillOverrides`, etc.). Anything **≥ 2.1.126** is safe.

If npm complains about permissions when writing to a global path:

If you used `brew install node` from Step 4, you generally won't hit this — Homebrew owns its own prefix. The workaround below is for users who installed `node` from the Node.js installer or another non-Homebrew source.

```bash
# Option A: user-local npm global prefix (only if you didn't use Homebrew node).
mkdir -p ~/.npm-global
npm config set prefix ~/.npm-global
echo 'export PATH=$HOME/.npm-global/bin:$PATH' >> ~/.zprofile
exec zsh -l   # re-login so ~/.zprofile re-runs
npm install -g @anthropic-ai/claude-code

# Option B: switch to Homebrew Node.
brew install node   # if not already done in Step 4
# then re-run the npm install
```

Verify:

```bash
claude --version    # expect: 2.1.126 or newer
```

---

## Step 6 — Install Rust toolchain

```bash
# Skip if `rustc --version` already works and reports >= 1.85.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
source "$HOME/.cargo/env"

rustc --version   # expect: rustc 1.85+ ...
cargo --version
```

> **Minimum supported Rust is 1.85** (edition 2024). rustup's `stable` channel has been on 1.85+ since early 2026 — no nightly required.

The installer offers to modify `~/.zshenv`, `~/.zshrc`, etc. on its own; accept the default. If you decline and `cargo` isn't found in new shells, add `. "$HOME/.cargo/env"` to `~/.zprofile`.

---

## Step 7 — Clone the fork, init submodules, build

```bash
# Pick a parent directory. The repo will land in $PARENT/cas.
mkdir -p ~/code && cd ~/code

# Clone the fork (HTTPS — switch to SSH if you've set up keys).
git clone https://github.com/Richards-LLC/cassy.git
cd cassy

# CRITICAL: pull the vendored Ghostty submodule. Build panics without this with
# "Ghostty submodule not found at vendor/ghostty".
git submodule update --init --recursive

# Build the release binary. First build is 5–15 min (compiles ~600 crate deps + Zig-builds libghostty-vt).
cargo build -p cas --profile release-fast

# Install to ~/.local/bin/ (no sudo). Create the dir if it doesn't exist.
mkdir -p ~/.local/bin
install -m 0755 target/release-fast/cas ~/.local/bin/cas
```

Add `~/.local/bin` to PATH if it isn't already:

```bash
echo $PATH | tr ':' '\n' | grep -q "$HOME/.local/bin" || {
  echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.zprofile
  exec zsh -l   # re-login so ~/.zprofile re-runs
}
```

Verify:

```bash
cas --version       # expect: cas 2.15.0 (or whatever HEAD is on the fork)
which cas           # expect: /Users/you/.local/bin/cas
cas doctor          # diagnostics; should report all green
```

> **Why `release-fast`, not `release`:** the `release-fast` profile uses thin LTO + 16 codegen units and strips debuginfo only — first build is ~30% faster than full `release`, and the runtime difference for CAS is negligible. Use `--release` if you want the smaller binary.

---

## Step 8 — Wire CAS into Claude Code (MCP)

CAS exposes its memory/task/skill APIs to Claude Code over MCP (stdio). Two scopes; pick one:

- **Project scope** (recommended for getting started): write the config to `<your-project>/.mcp.json`. Claude Code picks it up only when run from that project. Travels with the repo if checked in.
- **User scope** (cross-project): write it to `~/.claude.json`. If `~/.claude.json` doesn't exist, run `claude mcp list` once to let Claude Code scaffold it.

Either way, the `mcpServers` block looks the same:

```json
{
  "mcpServers": {
    "cas": {
      "command": "cas",
      "args": ["serve"]
    }
  }
}
```

Then in Claude Code, run `/mcp` — `cas` should appear and connect within ~1 second. If it's red ("disconnected"), the most likely cause is `cas` not being on the `$PATH` of the shell that Claude Code spawns. Fix Step 7's PATH export, or hard-code the absolute path:

```json
{ "mcpServers": { "cas": { "command": "/Users/you/.local/bin/cas", "args": ["serve"] } } }
```

---

## Step 9 — Initialize CAS in a project

```bash
cd /path/to/your/repo
cas init
```

This creates a `.cas/` directory with `cas.db` (SQLite for memories/tasks/rules/skills) and a `config.toml`. It also writes a starter `.claude/` directory with built-in skills, agents, and settings.

```bash
ls .cas/
# expect: cas.db  config.toml  cache/  backup/

ls .claude/
# expect: skills/  agents/  rules/  settings.json
```

---

## Step 10 — First factory session

```bash
cas        # launches the factory TUI
# or
cas -w 3   # launches with 3 worker panes pre-allocated
```

You should see the factory UI: a supervisor pane on the left, worker panes on the right. Type a task into the supervisor input ("look around the codebase and tell me what's here") and the supervisor will spawn a worker to do it.

To exit cleanly: `Ctrl-Q` from the supervisor pane.

---

## Upgrading later

Because you built from source, there's no auto-update — pull and rebuild:

```bash
cd ~/code/cas
git fetch origin
git pull origin main
git submodule update --init --recursive   # if the submodule moved
cargo build -p cas --profile release-fast
install -m 0755 target/release-fast/cas ~/.local/bin/cas

cas --version   # confirm new build
```

Subsequent rebuilds are seconds (incremental cargo), unless `vendor/ghostty` changed — then libghostty-vt re-links via Zig (~30 s).

---

## Where things live (Mac edition)

| What | Path |
|---|---|
| `cas` binary | `~/.local/bin/cas` |
| Source checkout | `~/code/cas/` (wherever you cloned) |
| Build artifacts | `~/code/cas/target/release-fast/` |
| Vendored Zig (if you ran `./scripts/bootstrap-zig.sh`) | `~/code/cas/.context/zig/zig` (gitignored) |
| Global CAS data | `~/.config/cas/` (XDG-style, even on Mac — CAS doesn't use `~/Library/Application Support`) |
| Per-project CAS data | `<project>/.cas/` |
| Per-project Claude config | `<project>/.claude/` |
| MCP config (project) | `<project>/.mcp.json` |
| MCP config (user) | `~/.claude.json` |
| Claude Code itself | `/opt/homebrew/bin/claude` (Homebrew node) or `~/.npm-global/bin/claude` (user-prefix workaround) |

---

## Troubleshooting

### `Ghostty submodule not found at vendor/ghostty` during `cargo build`

You skipped `git submodule update --init --recursive` after the clone. Run it from the repo root and re-run `cargo build`.

### `Zig compiler not found` during `cargo build`

`crates/ghostty_vt_sys/build.rs` can't find Zig. Fix by either:
- `brew install zig` (recommended), or
- `./scripts/bootstrap-zig.sh` from the repo root (downloads Zig 0.15.2 into `.context/zig/`), or
- export `ZIG=/path/to/zig` before `cargo build`.

### `linker 'cc' not found` or `ld: library not found for -lSystem`

Xcode Command Line Tools missing or incomplete. Re-run `xcode-select --install` (Step 3).

### `command not found: cas` after install

`~/.local/bin` is not on `$PATH`. Run:
```bash
echo $PATH | tr ':' '\n' | grep -E '\.local/bin'
```
If nothing prints, redo the PATH export in Step 7 and run `exec zsh -l` (or open a new terminal tab).

### `cas: bad CPU type in executable`

You built for the wrong architecture (e.g., copied a Linux binary, or built on Intel and ran on Apple Silicon). Rebuild from source on the target machine.

### `<Box> can't be nested inside <Text>` crash in Claude Code

You're on Claude Code in the broken 2.1.117 – 2.1.125 range. Anthropic fixed this in **2.1.126**; upgrade forward:
```bash
npm install -g @anthropic-ai/claude-code@latest
claude --version    # confirm >= 2.1.126
```

### `cas serve` works in terminal but Claude Code shows MCP disconnected

The shell Claude Code spawns has a different `$PATH` than your interactive zsh. Either:
- Use the absolute path in `.mcp.json` (`"command": "/Users/you/.local/bin/cas"`), or
- Move your `$PATH` exports from `~/.zprofile` to `~/.zshenv` so they apply to non-interactive shells too.

### Gatekeeper popup: `"cas" cannot be opened because the developer cannot be verified`

You built locally, so Gatekeeper shouldn't be involved — but if you copied the binary via AirDrop, Mail, or Safari, the quarantine attribute may have been set:
```bash
xattr -d com.apple.quarantine $(which cas) 2>/dev/null || true
```

### Build fails with `panic = "abort"` is not supported

You're on a custom Cargo profile that overrides `panic`. The MCP panic catcher requires `panic = "unwind"` — don't override it. See `cas-cli/src/lib.rs` for the compile-time guard and CLAUDE.md for the rationale.

---

## What this guide does NOT cover

- **Legacy public install paths** (`install.sh`, Homebrew). They ship a much older binary; use this repository's build instead.
- **CAS Cloud sync setup beyond the basics** — `cas login` + `cas cloud sync` are packaged commands; team scope auto-resolves from your `/api/me` membership. See the [README Team Memories section](../../README.md#team-memories-optional) for the full flow (`cas cloud team default <slug>` if you want to pin a team override).
- **`cas-update` / `cas-refresh` orchestrator scripts** — author-specific wrappers in `~/.local/bin/`. See `docs/ideation/2026-04-30-cas-shell-helpers-distribution-ideation.md`.
- **Multi-user setups**, shared `cas.db`, team collaboration patterns.
- **Custom skill / agent authoring**.
