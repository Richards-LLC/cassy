#!/usr/bin/env bash
# migration/phase2-prepare-target.sh
#
# Phase 2 of the Petrastella → Hetzner migration (epic cas-28d4, task cas-b5f1).
# Idempotent remote setup of the target server. Runs LOCALLY on the operator
# machine, drives the server via SSH. No project files are touched.
#
# Deliverables:
#   - Installs apt packages, rust toolchain, cas v2.0.x binary on the server
#   - Leaves pre-existing ~/.config/cas/env untouched (do NOT overwrite)
#   - Installs cas-serve@.service systemd unit and enables cas-serve@daniel
#   - Generates a GitHub SSH keypair (registration is human-in-the-loop)
#   - Runs a verification pass and appends all findings to phase2-log.md
#
# Every step is guarded by a "is this already done?" check so re-running the
# script is safe and produces "SKIP already-present" entries instead of
# re-installing.
#
# Usage:
#   ./migration/phase2-prepare-target.sh                    # run with defaults
#   REMOTE_HOST=daniel@1.2.3.4 ./migration/phase2-prepare-target.sh
#   LOG_FILE=/tmp/run.md       ./migration/phase2-prepare-target.sh
#
# CAS_SERVE_TOKEN is never written to any committed file. If the script
# generates one (only when ~/.config/cas/env is absent), it writes it to the
# remote env file over SSH and logs '<redacted>' locally.

set -euo pipefail
export LC_ALL=C

###############################################################################
# Config
###############################################################################

REMOTE_HOST="${REMOTE_HOST:-daniel@87.99.156.244}"
REMOTE_USER="${REMOTE_USER:-daniel}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="${LOG_FILE:-$SCRIPT_DIR/phase2-log.md}"
UNIT_FILE_LOCAL="${UNIT_FILE_LOCAL:-$SCRIPT_DIR/systemd/cas-serve@.service}"

# Required cas version prefix — must match what we install
REQUIRED_CAS_MAJOR=2

# Packages installed at Step 1. Keep sorted for idempotent log output.
APT_PACKAGES=(
  build-essential
  ca-certificates
  curl
  docker-compose-v2
  docker.io
  ffmpeg
  git
  jq
  libssl-dev
  pkg-config
  poppler-utils
  python3
  python3-pip
  python3-venv
  sqlite3
  unzip
  wget
)

###############################################################################
# Preconditions
###############################################################################

die() { printf 'phase2-prepare-target.sh: %s\n' "$*" >&2; exit 1; }

command -v ssh >/dev/null  || die "ssh is required"
command -v openssl >/dev/null || die "openssl is required"

###############################################################################
# Logging helpers
###############################################################################

# stderr progress for the operator running the script interactively
progress() { printf '[phase2] %s\n' "$*" >&2; }

# Redact any token-looking content before it hits the log. Two categories:
#   1. A raw 64-char hex run — the shape produced by `openssl rand -hex 32`.
#   2. Any flag or prefix explicitly labeled with "token" or "TOKEN" followed
#      by a value.
# We apply this filter to EVERYTHING written via `md` / `md_block`, so even
# unexpected capture paths (systemctl status showing the ExecStart line,
# journalctl replaying the bridge server's startup banner, etc.) cannot leak.
# Secrets hygiene is P0 for this task; accept a few false positives on
# unrelated long hex runs.
redact_secrets() {
  # Four defensive patterns. Order matters: the hex rule runs first as the
  # broadest catch, then the labeled rules cover any non-hex encodings that
  # the hex rule would miss (e.g. base64, uppercase, or a format change in
  # the cas binary).
  #
  #   1. Any 32+ char run of case-insensitive hex → `openssl rand -hex 32`
  #      produces 64 chars; `32,` is generous in case of partial-prefix logs.
  #   2. `CAS_SERVE_TOKEN=<value>` in env files or shell lines.
  #   3. `--token <value>` in any CLI-form capture (systemctl status, ps).
  #   4. `Token: <value>` banner from the cas bridge server's startup output —
  #      uses `[^[:space:]]{8,}` so the match covers hex, base64, and any
  #      future encoding regardless of character set.
  sed -E '
    s/[a-fA-F0-9]{32,}/<redacted-token>/g
    s/(CAS_SERVE_TOKEN=)[^ ]+/\1<redacted>/g
    s/(--token[[:space:]]+)[^[:space:]]+/\1<redacted>/g
    s/(Token:[[:space:]]*)[^[:space:]]{8,}/\1<redacted>/gi
  '
}

# markdown to the log file (sanitized)
md() { printf '%s\n' "$*" | redact_secrets >> "$LOG_FILE"; }
md_block() {
  local lang="${1:-}"
  printf '```%s\n' "$lang" >> "$LOG_FILE"
  redact_secrets >> "$LOG_FILE"
  printf '```\n' >> "$LOG_FILE"
}

ts() { date -u '+%Y-%m-%dT%H:%M:%SZ'; }

init_log() {
  cat > "$LOG_FILE" <<EOF
# Phase 2 execution log — prepare Hetzner target

**Task:** cas-b5f1
**Epic:** cas-28d4 (Petrastella → Hetzner migration)
**Host:** \`$REMOTE_HOST\`
**Script:** \`migration/phase2-prepare-target.sh\`
**Started:** $(ts)

This log is produced by \`phase2-prepare-target.sh\` and committed to cas-src
as the audit trail for the Phase 2 run. It's structured as one section per
step so a reader can jump to the step they care about. Steps that found the
server already in the desired state print \`SKIP\` and a short reason.

Secrets hygiene: \`CAS_SERVE_TOKEN\` never appears in this file. Where it would,
\`<redacted>\` is written instead. The token itself only exists in
\`~/.config/cas/serve.env\` on the remote (mode 0600, owner daniel:daniel) —
deliberately SEPARATE from the pre-existing \`~/.config/cas/env\` user token
vault (GH_TOKEN, VERCEL_TOKEN, etc.) per supervisor's option-(b) resolution.

EOF
}

###############################################################################
# SSH helpers
###############################################################################

# Run a command on the remote. Echoes output to stderr for the operator and
# captures it to stdout so callers can consume it. If the command fails,
# returns the non-zero exit status unchanged (do NOT mask — step functions
# decide whether failure is fatal).
remote() {
  ssh -o ConnectTimeout=15 -o BatchMode=yes "$REMOTE_HOST" "$*" 2>&1
}

# Run a command on the remote and tee its output into the log file under a
# markdown code fence. Returns the command's exit status.
remote_tee() {
  local cmd="$*"
  local out
  local rc=0
  out=$(remote "$cmd") || rc=$?
  printf '%s\n' "$out" | md_block
  return "$rc"
}

###############################################################################
# Step 0 — pre-flight
###############################################################################

step0_preflight() {
  progress "Step 0: pre-flight"
  md "## Step 0 — pre-flight"
  md ""
  md "Gates that must all pass before the script will touch the server."
  md ""

  local whoami_out sudo_out uname_out
  whoami_out=$(remote 'whoami') || die "ssh to $REMOTE_HOST failed (whoami exit $?)"
  sudo_out=$(remote 'sudo -n true && echo sudo_ok' 2>&1) || die "passwordless sudo check failed: $sudo_out"
  uname_out=$(remote 'uname -srm')

  md "- \`whoami\` → \`$whoami_out\`"
  md "- \`sudo -n true\` → \`$sudo_out\`"
  md "- \`uname -srm\` → \`$uname_out\`"
  md ""

  [ "$whoami_out" = "$REMOTE_USER" ] || die "expected whoami=$REMOTE_USER, got $whoami_out"
  echo "$uname_out" | grep -q 'Linux.*x86_64' || die "expected Linux x86_64, got: $uname_out"

  md "All gates **PASS**."
  md ""
}

###############################################################################
# Step 1 — apt packages
###############################################################################

step1_apt() {
  progress "Step 1: apt packages"
  md "## Step 1 — apt packages"
  md ""
  md "Required packages (sorted, deduped):"
  md ""
  for pkg in "${APT_PACKAGES[@]}"; do md "- \`$pkg\`"; done
  md ""

  # Determine missing packages with one round trip
  local pkg_list
  pkg_list=$(printf '%s ' "${APT_PACKAGES[@]}")
  local missing
  missing=$(remote "
    set -e
    missing=
    for p in $pkg_list; do
      if ! dpkg -s \"\$p\" >/dev/null 2>&1; then
        missing=\"\$missing \$p\"
      fi
    done
    echo \"\$missing\" | tr -s ' '
  ")

  missing=$(echo "$missing" | xargs)  # trim

  if [ -z "$missing" ]; then
    md "All $((${#APT_PACKAGES[@]})) packages already installed — **SKIP**."
    md ""
    progress "  ↳ SKIP (all packages already installed)"
    return 0
  fi

  md "Missing packages: \`$missing\`"
  md ""
  md "Running \`sudo apt-get update && sudo apt-get install -y $missing\`:"
  md ""

  progress "  ↳ installing: $missing"
  remote_tee "sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq 2>&1 | tail -5 && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $missing 2>&1 | tail -20" || {
    md ""
    md "**apt install FAILED** — see output above. Continuing with subsequent steps; individual tool checks later will catch missing dependencies."
    md ""
    progress "  ↳ apt install returned non-zero (see log)"
    return 0
  }

  md ""
}

###############################################################################
# Step 2 — Node.js (check-only; server already has Node 22)
###############################################################################

step2_node() {
  progress "Step 2: node + pnpm"
  md "## Step 2 — Node.js + pnpm"
  md ""

  local node_ver pnpm_ver
  node_ver=$(remote 'node --version 2>/dev/null || echo MISSING')
  pnpm_ver=$(remote 'pnpm --version 2>/dev/null || echo MISSING')

  md "- \`node --version\` → \`$node_ver\`"
  md "- \`pnpm --version\` → \`$pnpm_ver\`"
  md ""

  if [ "$node_ver" = "MISSING" ] || [ "$pnpm_ver" = "MISSING" ]; then
    md "**Node/pnpm missing** — installing via nvm + corepack."
    md ""
    remote_tee '
      set -e
      if ! command -v node >/dev/null 2>&1; then
        curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
        export NVM_DIR="$HOME/.nvm"
        [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
        nvm install --lts
        nvm use --lts
      fi
      if ! command -v pnpm >/dev/null 2>&1; then
        corepack enable
        corepack prepare pnpm@latest --activate
      fi
      node --version
      pnpm --version
    '
  else
    # Version floor: Node major >= 20
    local major
    major=$(printf '%s' "$node_ver" | sed -E 's/^v([0-9]+).*/\1/')
    if [ "$major" -lt 20 ]; then
      md "**Node $node_ver is below the required v20+** — please investigate before proceeding."
      md ""
      die "Node too old: $node_ver"
    fi
    md "Node $node_ver and pnpm $pnpm_ver both present and satisfy the v20+ floor — **SKIP**."
    md ""
    progress "  ↳ SKIP (node=$node_ver pnpm=$pnpm_ver)"
  fi
}

###############################################################################
# Step 3 — Rust toolchain
###############################################################################

step3_rust() {
  progress "Step 3: rust toolchain"
  md "## Step 3 — Rust toolchain (rustup + cargo)"
  md ""

  local rustc_ver cargo_ver
  rustc_ver=$(remote 'source "$HOME/.cargo/env" 2>/dev/null; rustc --version 2>/dev/null || echo MISSING')
  cargo_ver=$(remote 'source "$HOME/.cargo/env" 2>/dev/null; cargo --version 2>/dev/null || echo MISSING')

  md "- \`rustc --version\` → \`$rustc_ver\`"
  md "- \`cargo --version\` → \`$cargo_ver\`"
  md ""

  if [ "$rustc_ver" = "MISSING" ] || [ "$cargo_ver" = "MISSING" ]; then
    md "Installing rustup (stable toolchain, default profile)…"
    md ""
    progress "  ↳ installing rustup stable"
    remote_tee "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile default 2>&1 | tail -20"
    rustc_ver=$(remote 'source "$HOME/.cargo/env" 2>/dev/null; rustc --version')
    cargo_ver=$(remote 'source "$HOME/.cargo/env" 2>/dev/null; cargo --version')
    md ""
    md "Installed: \`$rustc_ver\` / \`$cargo_ver\`"
    md ""
  else
    md "rustc and cargo already present — **SKIP**."
    md ""
    progress "  ↳ SKIP ($rustc_ver / $cargo_ver)"
  fi
}

###############################################################################
# Step 4 — cas binary (v2.0.x)
###############################################################################

step4_cas_binary() {
  progress "Step 4: cas v2 binary"
  md "## Step 4 — cas binary (v$REQUIRED_CAS_MAJOR.0.x)"
  md ""

  local cas_ver
  cas_ver=$(remote 'cas --version 2>/dev/null || echo MISSING')
  md "- \`cas --version\` (current) → \`$cas_ver\`"
  md ""

  if echo "$cas_ver" | grep -qE "^cas ${REQUIRED_CAS_MAJOR}\."; then
    md "cas v${REQUIRED_CAS_MAJOR}.x already installed — **SKIP**."
    md ""
    progress "  ↳ SKIP ($cas_ver)"
    return 0
  fi

  md "Current version is not v${REQUIRED_CAS_MAJOR}.x. Installing."
  md ""
  md "\`Richards-LLC/cassy\` has no GitHub Releases yet (\`GET /repos/Richards-LLC/cassy/releases/latest\` returns 404), so \`scripts/cas-install.sh\` cannot resolve a tag. Falling back to **build from source** per the spec: clone \`Richards-LLC/cassy\` on the server to \`~/src/cas-src\`, \`cargo install --path cas-cli --force\`, and symlink into \`/usr/local/bin/cas\`."
  md ""

  progress "  ↳ building cas v2 from source on server (this takes ~5-8 min on CCX23)"
  remote_tee '
    set -e
    source "$HOME/.cargo/env"
    mkdir -p "$HOME/src"
    cd "$HOME/src"
    if [ ! -d cas-src ]; then
      git clone --depth 50 https://github.com/Richards-LLC/cassy.git cas-src
    fi
    cd cas-src
    git fetch origin main
    git checkout main
    git reset --hard origin/main
    git log -1 --oneline
    cargo install --path cas-cli --force 2>&1 | tail -20
    NEW_BIN="$HOME/.cargo/bin/cas"
    "$NEW_BIN" --version
    if [ -w /usr/local/bin ] || command -v sudo >/dev/null 2>&1; then
      sudo install -m 755 "$NEW_BIN" /usr/local/bin/cas
      /usr/local/bin/cas --version
    fi
  '

  cas_ver=$(remote 'cas --version')
  md ""
  md "Post-install: \`$cas_ver\`"
  md ""
  echo "$cas_ver" | grep -qE "^cas ${REQUIRED_CAS_MAJOR}\." || die "cas install did not produce v${REQUIRED_CAS_MAJOR}.x: $cas_ver"
}

###############################################################################
# Step 5 — ~/.config/cas/env
###############################################################################

step5_cas_env() {
  progress "Step 5: ~/.config/cas/serve.env (separate-file deviation from spec)"
  md "## Step 5 — \`~/.config/cas/serve.env\` (separate from user token vault)"
  md ""
  md "**Spec deviation (authorized by supervisor on cas-b5f1 at 2026-04-11T~13:20):**"
  md ""
  md "The task spec's Step 5 targets \`~/.config/cas/env\` as the systemd-unit \`EnvironmentFile\`. On this server that path is already occupied by \`cas-fb43\`'s provisioning output — a user-level **login-shell token vault** containing \`GH_TOKEN\`, \`GITHUB_TOKEN\`, \`CAS_CLOUD_TOKEN\`, \`CONTEXT7_API_KEY\`, \`NEON_API_KEY\`, \`VERCEL_TOKEN\`, \`BROWSERLESS_API_KEY\`, etc. These are the user's CLI-tool credentials, likely sourced by \`.bashrc\`/\`.zshrc\`, and should NOT be inherited by a system-level HTTP bridge process."
  md ""
  md "**Resolution** (option (b) from the blocker note): a separate file \`~/.config/cas/serve.env\` scoped strictly to the systemd unit. The user's token vault at \`~/.config/cas/env\` is **never touched**. Consequences:"
  md ""
  md "- Principle of least privilege honored: \`cas-serve@daniel\` doesn't see \`NEON_API_KEY\`, \`VERCEL_TOKEN\`, etc."
  md "- Future changes to the user token vault no longer implicitly mutate the service environment."
  md "- The committed \`migration/systemd/cas-serve@.service\` uses \`EnvironmentFile=/home/%i/.config/cas/serve.env\` (spec said \`.../env\` — the deviation is documented here, in the unit file's header comment, and in the close reason)."
  md ""

  local existing_state
  existing_state=$(remote '
    if [ -f "$HOME/.config/cas/env" ]; then
      echo "TOKEN_VAULT_PRESENT"
      stat -c "mode=%a owner=%U:%G size=%s" "$HOME/.config/cas/env"
    else
      echo "TOKEN_VAULT_ABSENT"
    fi
    if [ -f "$HOME/.config/cas/serve.env" ]; then
      echo "SERVE_ENV_PRESENT"
      stat -c "mode=%a owner=%U:%G size=%s" "$HOME/.config/cas/serve.env"
      if grep -qE "^(export +)?CAS_SERVE_URL=" "$HOME/.config/cas/serve.env" 2>/dev/null; then echo has_CAS_SERVE_URL; else echo no_CAS_SERVE_URL; fi
      if grep -qE "^(export +)?CAS_SERVE_TOKEN=" "$HOME/.config/cas/serve.env" 2>/dev/null; then echo has_CAS_SERVE_TOKEN; else echo no_CAS_SERVE_TOKEN; fi
    else
      echo "SERVE_ENV_ABSENT"
    fi
  ')

  md "Remote state probe (user token vault left untouched throughout):"
  md ""
  printf '%s\n' "$existing_state" | md_block
  md ""

  local serve_env_has_both=no
  if echo "$existing_state" | grep -q has_CAS_SERVE_URL && echo "$existing_state" | grep -q has_CAS_SERVE_TOKEN; then
    serve_env_has_both=yes
  fi

  if [ "$serve_env_has_both" = "yes" ]; then
    md "\`serve.env\` already present with both required keys — **SKIP**."
    md ""
    progress "  ↳ SKIP (serve.env already has CAS_SERVE_URL + CAS_SERVE_TOKEN)"
    return 0
  fi

  md "Creating a fresh \`serve.env\` with \`CAS_SERVE_URL\` + a new 32-byte token. The token is piped via SSH stdin so it never lands in a local temp file or this log."
  md ""

  local token
  token=$(openssl rand -hex 32)

  # Write the file via SSH stdin. `cat >` on the remote with the heredoc
  # semantics written inside a ssh single-quoted string is safer than
  # expanding $token in the outer shell's heredoc.
  printf 'CAS_SERVE_URL=http://127.0.0.1:18999\nCAS_SERVE_TOKEN=%s\n' "$token" | \
    ssh -o BatchMode=yes "$REMOTE_HOST" '
      set -e
      mkdir -p "$HOME/.config/cas"
      umask 077
      cat > "$HOME/.config/cas/serve.env"
      chmod 600 "$HOME/.config/cas/serve.env"
      chown "$USER:$USER" "$HOME/.config/cas/serve.env"
    ' >/dev/null

  token=""
  unset token

  remote '
    stat -c "mode=%a owner=%U:%G size=%s" "$HOME/.config/cas/serve.env"
    echo "CAS_SERVE_URL present: $(grep -c ^CAS_SERVE_URL= $HOME/.config/cas/serve.env)"
    echo "CAS_SERVE_TOKEN present: $(grep -c ^CAS_SERVE_TOKEN= $HOME/.config/cas/serve.env)"
    echo "Line count: $(wc -l < $HOME/.config/cas/serve.env)"
  ' | md_block

  md ""
  md "Created \`serve.env\` (mode 0600, owner daniel:daniel). Token: \`CAS_SERVE_TOKEN=<redacted>\`"
  md ""
  md "Sanity check — the user token vault at \`~/.config/cas/env\` remains untouched:"
  md ""
  remote 'stat -c "mode=%a owner=%U:%G size=%s mtime=%y" "$HOME/.config/cas/env" 2>/dev/null || echo "user vault absent"' | md_block
  md ""
  progress "  ↳ CREATED ~/.config/cas/serve.env (token redacted)"
}

###############################################################################
# Step 6 — systemd unit cas-serve@.service
###############################################################################

step6_systemd() {
  progress "Step 6: cas-serve@.service systemd unit"
  md "## Step 6 — systemd unit \`cas-serve@.service\`"
  md ""

  md "**Correction from the original spec:** the spec template used \`cas serve --port 18999\` as the ExecStart, but on the installed \`cas 2.0.0\` binary \`cas serve\` is the stdio MCP server for Claude Code integration (\`--json\`/\`--full\`/\`--verbose\` only — no port binding)."
  md ""
  md "The HTTP bridge we actually need is \`cas bridge serve\`. Supervisor authorized adjusting ExecStart after probing \`--help\` on the newly-installed binary, so this unit uses the correct subcommand."
  md ""
  md "Probing \`cas bridge serve --help\` on the server to lock the exact flag names:"
  md ""
  local help_out
  help_out=$(remote 'cas bridge serve --help 2>&1')
  printf '%s\n' "$help_out" | md_block
  md ""

  # Decide ExecStart from the help output. All three flags MUST be present —
  # we do NOT silently fall back to --no-auth, because that would write an
  # authentication-less unit file to disk, commit it to the worktree, and
  # push it to the server without anyone noticing. Instead we abort so a
  # human can assess whether the cas CLI has legitimately changed or whether
  # the probe is wrong.
  local exec_start
  if echo "$help_out" | grep -q -- '--bind' && echo "$help_out" | grep -q -- '--port' && echo "$help_out" | grep -q -- '--token'; then
    # Systemd expands ${CAS_SERVE_TOKEN} from EnvironmentFile-loaded variables.
    # The ${VAR} expansion in ExecStart is a documented systemd.exec feature.
    exec_start='/usr/local/bin/cas bridge serve --bind 127.0.0.1 --port 18999 --token ${CAS_SERVE_TOKEN}'
    md "\`--bind\` + \`--port\` + \`--token\` flags confirmed. Using systemd \`\${CAS_SERVE_TOKEN}\` expansion from the EnvironmentFile so the token never appears literally in the unit file (the unit file is committed to git)."
  else
    md "**UNEXPECTED \`cas bridge serve --help\` SHAPE — ABORTING.**"
    md ""
    md "Expected all three of \`--bind\`, \`--port\`, \`--token\` to be present in the help output. At least one is missing. This script will NOT silently fall back to \`--no-auth\`: that would write an authentication-less HTTP bridge unit to \`/etc/systemd/system/\` without anyone noticing. Aborting now so a human can assess whether the cas CLI has legitimately changed or whether the probe is wrong."
    md ""
    die "cas bridge serve --help does not expose --bind/--port/--token; refusing to generate a no-auth unit. Inspect the help output in phase2-log.md and update step6_systemd's flag check once you've decided on the correct ExecStart."
  fi
  md ""

  # Write the unit file to the worktree FIRST (canonical versioned copy)
  mkdir -p "$(dirname "$UNIT_FILE_LOCAL")"
  cat > "$UNIT_FILE_LOCAL" <<EOF
# migration/systemd/cas-serve@.service
#
# Versioned systemd unit template for the cas HTTP bridge server. Phase 2
# of the Petrastella → Hetzner migration (cas-b5f1) installs this file to
# /etc/systemd/system/cas-serve@.service on daniel@87.99.156.244 and enables
# cas-serve@daniel. Edit this file in cas-src; re-run
# migration/phase2-prepare-target.sh to push updates to the server.
#
# Spec deviation: the task spec originally called for
#   EnvironmentFile=/home/%i/.config/cas/env
# but on the target server \`.config/cas/env\` is already occupied by
# cas-fb43's user-level token vault (GH_TOKEN, VERCEL_TOKEN, NEON_API_KEY,
# etc. — login-shell CLI creds). Per supervisor's option-(b) resolution on
# cas-b5f1, this unit uses a separate file \`serve.env\` scoped strictly
# to the systemd service so principle of least privilege is honored
# (cas-serve never inherits user CLI tokens) and future vault edits don't
# implicitly mutate the service environment.
#
# ExecStart uses \`cas bridge serve\` (not \`cas serve\`, which is the
# stdio MCP subcommand) and reads the bearer token from serve.env via
# systemd's \${VAR} expansion so the token is never literalized into
# this committed file.

[Unit]
Description=CAS HTTP bridge server for %i
After=network.target
AssertPathExists=/home/%i/.config/cas/serve.env

[Service]
Type=simple
User=%i
Group=%i
WorkingDirectory=/home/%i
EnvironmentFile=/home/%i/.config/cas/serve.env
ExecStart=$exec_start
Restart=on-failure
RestartSec=5

# Discard stdout so the cas startup banner's 'Token: <value>' line does NOT
# land in the systemd journal. The banner is informational and the token is
# already in serve.env for anyone who needs it. Errors still flow through
# stderr → journal for real operational signal.
#
# NOTE: /proc/<pid>/cmdline still exposes the token via systemd's
# \${CAS_SERVE_TOKEN} argv expansion. That's a cas-cli limitation (the
# --token flag is the only auth mechanism the binary accepts). On a
# single-user server (daniel only), the exposure is bounded to daniel +
# root. A cas-binary follow-up to add CAS_SERVE_TOKEN env-var reading would
# close this hole completely.
StandardOutput=null
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

  md "Local unit file written to \`migration/systemd/cas-serve@.service\`:"
  md ""
  md_block ini < "$UNIT_FILE_LOCAL"
  md ""

  # Push to server. Use sudo tee to write to /etc/systemd/system.
  progress "  ↳ installing unit on server"
  local installed_md5 local_md5
  local_md5=$(md5sum "$UNIT_FILE_LOCAL" | awk '{print $1}')
  installed_md5=$(remote 'sudo test -f /etc/systemd/system/cas-serve@.service && sudo md5sum /etc/systemd/system/cas-serve@.service | awk "{print \$1}" || echo ABSENT')

  if [ "$installed_md5" = "$local_md5" ]; then
    md "Server already has the identical unit file (md5 match). **SKIP write.**"
    md ""
    progress "  ↳ SKIP (server unit file identical)"
  else
    md "Server unit file md5 is \`$installed_md5\`, local md5 is \`$local_md5\`. Installing."
    md ""
    cat "$UNIT_FILE_LOCAL" | ssh "$REMOTE_HOST" 'sudo tee /etc/systemd/system/cas-serve@.service > /dev/null && sudo chmod 644 /etc/systemd/system/cas-serve@.service && sudo systemctl daemon-reload'
    md "Installed and \`systemctl daemon-reload\` ran."
    md ""
  fi

  # Enable + start cas-serve@daniel (idempotent — already-enabled/already-running is fine)
  progress "  ↳ enabling cas-serve@daniel"
  remote_tee "sudo systemctl enable --now cas-serve@$REMOTE_USER 2>&1; sudo systemctl is-active cas-serve@$REMOTE_USER 2>&1; sudo systemctl status cas-serve@$REMOTE_USER --no-pager 2>&1 | head -15"
  md ""
}

###############################################################################
# Step 7 — ~/projects/ directory
###############################################################################

step7_projects_dir() {
  progress "Step 7: ~/projects/"
  md "## Step 7 — \`~/projects/\` directory"
  md ""

  local state
  state=$(remote 'if [ -d "$HOME/projects" ]; then stat -c "mode=%a owner=%U:%G" "$HOME/projects"; else echo ABSENT; fi')
  md "- \`~/projects\` → \`$state\`"
  md ""

  if echo "$state" | grep -q ABSENT; then
    remote "mkdir -p \"\$HOME/projects\" && chown \$USER:\$USER \"\$HOME/projects\""
    md "Created \`~/projects\`."
    md ""
  else
    md "\`~/projects\` already exists — **SKIP**."
    md ""
    progress "  ↳ SKIP (present)"
  fi
}

###############################################################################
# Step 8 — GitHub SSH keygen (NOT registration)
###############################################################################

step8_sshkey() {
  progress "Step 8: GitHub SSH keypair"
  md "## Step 8 — GitHub SSH keypair (keygen only — NOT registration)"
  md ""

  local key_state
  key_state=$(remote 'if [ -f "$HOME/.ssh/id_ed25519_github" ] && [ -f "$HOME/.ssh/id_ed25519_github.pub" ]; then echo PRESENT; else echo ABSENT; fi')
  md "- \`~/.ssh/id_ed25519_github{,.pub}\` → \`$key_state\`"
  md ""

  if [ "$key_state" = "ABSENT" ]; then
    progress "  ↳ generating ed25519 keypair"
    remote "
      set -e
      mkdir -p \"\$HOME/.ssh\"
      chmod 700 \"\$HOME/.ssh\"
      ssh-keygen -t ed25519 -C 'daniel@hetzner-cas-87.99.156.244' -f \"\$HOME/.ssh/id_ed25519_github\" -N ''
      chmod 600 \"\$HOME/.ssh/id_ed25519_github\"
      chmod 644 \"\$HOME/.ssh/id_ed25519_github.pub\"
    " | md_block

    md ""
    md "Keypair generated. The **public** key is captured below (for supervisor to register with GitHub). The **private** key remains on the server only."
    md ""
  else
    md "Keypair already present — **SKIP** keygen."
    md ""
    progress "  ↳ SKIP (keypair present)"
  fi

  # Append the Host github.com block to ~/.ssh/config if not already present
  remote '
    config="$HOME/.ssh/config"
    touch "$config"
    chmod 600 "$config"
    if ! grep -q "^Host github.com" "$config"; then
      printf "\nHost github.com\n    IdentityFile ~/.ssh/id_ed25519_github\n    IdentitiesOnly yes\n" >> "$config"
      echo "ssh_config_UPDATED"
    else
      echo "ssh_config_already_has_github_block_SKIP"
    fi
  ' | md_block
  md ""

  # Capture the public key for the human-in-the-loop section at the end
  REGISTER_PUBKEY=$(remote 'cat "$HOME/.ssh/id_ed25519_github.pub"')
}

###############################################################################
# Step 9 — verification pass
###############################################################################

step9_verify() {
  progress "Step 9: verification"
  md "## Step 9 — verification pass"
  md ""
  md "Capturing the authoritative end-state of every component touched by the script. Each check is a single SSH round trip."
  md ""

  verify_row() {
    local label="$1" cmd="$2"
    local out
    out=$(remote "$cmd" 2>&1) || true
    md "**$label**"
    md ""
    printf '%s\n' "$out" | md_block
    md ""
  }

  verify_row "cas --version (server)"                   "cas --version"
  verify_row "cas --help (top level)"                   "cas --help 2>&1 | head -20"
  verify_row "cas task list --scope global --limit 1"   "cas task list --scope global --limit 1 2>&1 || echo 'non-zero exit was expected with empty store'"
  verify_row "systemctl is-active cas-serve@$REMOTE_USER" "sudo systemctl is-active cas-serve@$REMOTE_USER 2>&1"
  verify_row "systemctl status cas-serve@$REMOTE_USER"    "sudo systemctl status cas-serve@$REMOTE_USER --no-pager 2>&1 | head -15"
  verify_row "curl 127.0.0.1:18999/health"              "curl -sS -m 5 http://127.0.0.1:18999/health 2>&1 || echo 'health endpoint returned non-zero — any 4xx/5xx body would appear above; hang/ECONNREFUSED is the failure case'"
  verify_row "curl 127.0.0.1:18999/ (root)"             "curl -sS -m 5 -o /dev/null -w 'HTTP %{http_code}\\n' http://127.0.0.1:18999/ 2>&1 || echo 'no response'"
  verify_row "docker --version"                         "docker --version 2>&1"
  verify_row "docker compose version"                   "docker compose version 2>&1 || echo 'compose v2 plugin not present'"
  verify_row "node --version / pnpm --version"          "node --version && pnpm --version"
  verify_row "cargo --version"                          "source \$HOME/.cargo/env; cargo --version"
  verify_row "rustc --version"                          "source \$HOME/.cargo/env; rustc --version"
  verify_row "sqlite3 --version"                        "sqlite3 --version 2>&1 || echo MISSING"
  verify_row "~/projects listing"                       "ls -la \$HOME/projects/"
  verify_row "~/.config/cas/env — user token vault (untouched by this script, metadata only)" \
                                                         "stat -c 'mode=%a owner=%U:%G size=%s mtime=%y' \$HOME/.config/cas/env 2>&1 || echo 'absent'"
  verify_row "~/.config/cas/serve.env — systemd unit env file (presence of required keys)" \
                                                         "stat -c 'mode=%a owner=%U:%G size=%s' \$HOME/.config/cas/serve.env && printf 'CAS_SERVE_URL lines: %s\\n' \"\$(grep -c '^CAS_SERVE_URL=' \$HOME/.config/cas/serve.env)\" && printf 'CAS_SERVE_TOKEN lines: %s\\n' \"\$(grep -c '^CAS_SERVE_TOKEN=' \$HOME/.config/cas/serve.env)\""
  verify_row "id_ed25519_github private key (perms)"    "stat -c 'mode=%a owner=%U:%G' \$HOME/.ssh/id_ed25519_github"
  verify_row "id_ed25519_github.pub (perms + content)"  "stat -c 'mode=%a owner=%U:%G' \$HOME/.ssh/id_ed25519_github.pub && cat \$HOME/.ssh/id_ed25519_github.pub"
}

###############################################################################
# Human-in-the-loop section (SSH key registration)
###############################################################################

requires_human_section() {
  md "## Residual risk — token in argv (requires cas-cli follow-up)"
  md ""
  md "The \`cas bridge serve\` binary takes its bearer token via the \`--token <value>\` CLI flag; the Rust struct at \`cas-cli/src/cli/bridge.rs\` does not register \`CAS_SERVE_TOKEN\` as an env-var source for clap. As a result, systemd's \`\${CAS_SERVE_TOKEN}\` expansion puts the literal token into the process's \`argv\`, which means:"
  md ""
  md "- \`cat /proc/<pid>/cmdline | tr '\\\\0' ' '\` reveals the token to any local user/process with DAC read access."
  md "- \`systemctl status cas-serve@daniel\` / \`systemd-cgls\` / \`ps -ef\` show the full command line with the token."
  md ""
  md "On this host, daniel is the sole unprivileged user, so the exposure is bounded to daniel + root. The journal leak (cas prints \`Token: <value>\` at startup) has been mitigated in the unit file via \`StandardOutput=null\`, but the argv leak cannot be fixed at the systemd layer."
  md ""
  md "**Follow-up:** add \`#[arg(long, env = \"CAS_SERVE_TOKEN\")]\` to \`ServeArgs::token\` in \`cas-cli/src/cli/bridge.rs\` so the token can be read from the environment instead of CLI. Then remove \`--token \\\${CAS_SERVE_TOKEN}\` from the \`ExecStart\` line and rely on the \`EnvironmentFile=\` directive alone. This is a cas-cli change, not a migration-script change — tracking in the cas-28d4 epic close notes."
  md ""
  md "## REQUIRES HUMAN — register SSH key with GitHub"
  md ""
  md "The following public key lives on \`$REMOTE_HOST\` at \`~/.ssh/id_ed25519_github.pub\`. The supervisor (or user) must register it with GitHub manually before Phase 3 can clone repos over SSH on the server:"
  md ""
  md '```'
  md "${REGISTER_PUBKEY:-<capture failed — re-read from server with: ssh daniel@87.99.156.244 cat ~/.ssh/id_ed25519_github.pub>}"
  md '```'
  md ""
  md "Registration options:"
  md ""
  md "- \`gh ssh-key add ~/.ssh/id_ed25519_github.pub --title 'hetzner-cas-87.99.156.244'\` (run locally, not on the server)"
  md "- Or paste the above into https://github.com/settings/ssh/new with a clear title"
  md ""
  md "After registration, test from the server:"
  md ""
  md '```sh'
  md "ssh $REMOTE_HOST 'ssh -T -o StrictHostKeyChecking=accept-new git@github.com'"
  md '```'
  md ""
  md "Expected response: \`Hi <username>! You've successfully authenticated, but GitHub does not provide shell access.\`"
  md ""
}

###############################################################################
# Main
###############################################################################

main() {
  init_log
  progress "Writing log → $LOG_FILE"
  progress "Target: $REMOTE_HOST"

  step0_preflight
  step1_apt
  step2_node
  step3_rust
  step4_cas_binary
  step5_cas_env
  step6_systemd
  step7_projects_dir
  step8_sshkey
  step9_verify
  requires_human_section

  md "---"
  md ""
  md "**Completed:** $(ts)"
  md ""

  progress "Done. Log: $LOG_FILE"
}

main "$@"
