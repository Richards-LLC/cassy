#!/usr/bin/env bash
# provision-hetzner.sh — Idempotent provisioning for CAS development server
# Target: Hetzner CCX23 (Ubuntu, 16GB RAM, 150GB disk)
# Usage: Run as root on the target server, or via: ssh root@<ip> 'bash -s' < scripts/provision-hetzner.sh
set -euo pipefail

DANIEL_SSH_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGpA1xrG0zl8uYLVriPH4ptQCm98jpZET5pYqb93erqm pippenz@github"
NVM_VERSION="v0.40.3"
RUST_MIN="1.85"
CAS_REPO="https://github.com/Richards-LLC/cassy.git"
SWAP_SIZE="4G"

log() { echo -e "\n\033[1;34m>>> $1\033[0m"; }

# ── 1. System updates & essentials ──────────────────────────────────────────
log "System updates & essentials"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get upgrade -y -qq
apt-get install -y -qq \
  build-essential git curl unzip jq htop tmux \
  zsh python3 python3-pip python3-venv \
  postgresql-client \
  software-properties-common apt-transport-https \
  lld

# ── 2. Shell tools (system-wide): starship, eza, fzf, zoxide ───────────────
log "Shell tools"

# starship
if ! command -v starship &>/dev/null; then
  curl -fsSL https://starship.rs/install.sh | sh -s -- -y
fi

# eza (from official apt repo)
if ! command -v eza &>/dev/null; then
  mkdir -p /etc/apt/keyrings
  wget -qO- https://raw.githubusercontent.com/eza-community/eza/main/deb.asc | gpg --dearmor -o /etc/apt/keyrings/gierens.gpg 2>/dev/null || true
  echo "deb [signed-by=/etc/apt/keyrings/gierens.gpg] http://deb.gierens.de stable main" > /etc/apt/sources.list.d/gierens.list
  chmod 644 /etc/apt/keyrings/gierens.gpg /etc/apt/sources.list.d/gierens.list
  apt-get update -qq
  apt-get install -y -qq eza
fi

# fzf
if ! command -v fzf &>/dev/null; then
  apt-get install -y -qq fzf
fi

# zoxide
if ! command -v zoxide &>/dev/null; then
  curl -fsSL https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh | bash
  # zoxide installs to ~/.local/bin by default; move to /usr/local/bin for system-wide
  if [ -f /root/.local/bin/zoxide ]; then
    mv /root/.local/bin/zoxide /usr/local/bin/zoxide
  fi
fi

# ── 3. User accounts ───────────────────────────────────────────────────────
log "User accounts"

create_user() {
  local username=$1
  local ssh_key=${2:-""}
  if ! id "$username" &>/dev/null; then
    useradd -m -s /usr/bin/zsh -G sudo,users "$username"
    echo "$username ALL=(ALL) NOPASSWD:ALL" > "/etc/sudoers.d/$username"
    chmod 440 "/etc/sudoers.d/$username"
  else
    chsh -s /usr/bin/zsh "$username" 2>/dev/null || true
  fi
  # Ensure user dirs
  local home="/home/$username"
  mkdir -p "$home/.ssh" "$home/.cas" "$home/.claude" "$home/projects"
  if [ -n "$ssh_key" ]; then
    grep -qF "$ssh_key" "$home/.ssh/authorized_keys" 2>/dev/null || echo "$ssh_key" >> "$home/.ssh/authorized_keys"
    chmod 700 "$home/.ssh"
    chmod 600 "$home/.ssh/authorized_keys"
  fi
  chown -R "$username:$username" "$home"
}

create_user daniel "$DANIEL_SSH_KEY"
create_user ben ""

# ── 4. daniel's shell environment (.zshrc) ─────────────────────────────────
log "daniel's shell environment"

DANIEL_HOME="/home/daniel"

# oh-my-zsh
if [ ! -d "$DANIEL_HOME/.oh-my-zsh" ]; then
  su - daniel -c 'sh -c "$(curl -fsSL https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)" "" --unattended'
fi

# oh-my-zsh plugins
ZSH_CUSTOM="$DANIEL_HOME/.oh-my-zsh/custom"
if [ ! -d "$ZSH_CUSTOM/plugins/zsh-autosuggestions" ]; then
  su - daniel -c "git clone https://github.com/zsh-users/zsh-autosuggestions $ZSH_CUSTOM/plugins/zsh-autosuggestions"
fi
if [ ! -d "$ZSH_CUSTOM/plugins/zsh-syntax-highlighting" ]; then
  su - daniel -c "git clone https://github.com/zsh-users/zsh-syntax-highlighting $ZSH_CUSTOM/plugins/zsh-syntax-highlighting"
fi

# Write .zshrc
cat > "$DANIEL_HOME/.zshrc" << 'ZSHRC_EOF'
# ── oh-my-zsh ───────────────────────────────────────────────────────────────
export ZSH="$HOME/.oh-my-zsh"
ZSH_THEME=""  # using starship instead
plugins=(git zsh-autosuggestions zsh-syntax-highlighting)
source "$ZSH/oh-my-zsh.sh"

# ── Starship prompt ─────────────────────────────────────────────────────────
eval "$(starship init zsh)"

# ── eza aliases ──────────────────────────────────────────────────────────────
alias ls='eza --icons --color=always'
alias ll='eza -la --icons --color=always'

# ── CAS wrapper ──────────────────────────────────────────────────────────────
cas() {
  if [ $# -eq 0 ]; then
    command cas factory --new
  else
    command cas "$@"
  fi
}

cas-refresh() {
  echo ">>> Updating CAS..."
  cd ~/projects/cas && git pull && cargo build --release
  sudo cp target/release/cas /usr/local/bin/cas
  echo ">>> CAS login..."
  cas-login
  echo ">>> Cloud auth..."
  command cas cloud auth
  echo ">>> Cloud sync..."
  command cas cloud sync
  echo ">>> Starting factory..."
  command cas factory --new
}

# ── SSH agent persistent socket ──────────────────────────────────────────────
export SSH_AUTH_SOCK="$HOME/.ssh/agent.sock"
if ! ssh-add -l &>/dev/null 2>&1; then
  rm -f "$SSH_AUTH_SOCK"
  eval "$(ssh-agent -a "$SSH_AUTH_SOCK")" >/dev/null 2>&1
  ssh-add ~/.ssh/id_ed25519 2>/dev/null || true
fi

# ── nvm ──────────────────────────────────────────────────────────────────────
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
[ -s "$NVM_DIR/bash_completion" ] && source "$NVM_DIR/bash_completion"

# ── fzf ──────────────────────────────────────────────────────────────────────
[ -f /usr/share/doc/fzf/examples/key-bindings.zsh ] && source /usr/share/doc/fzf/examples/key-bindings.zsh
[ -f /usr/share/doc/fzf/examples/completion.zsh ] && source /usr/share/doc/fzf/examples/completion.zsh

# ── zoxide ───────────────────────────────────────────────────────────────────
eval "$(zoxide init zsh)"

# ── Python ───────────────────────────────────────────────────────────────────
alias python=python3

# ── Terminal title ───────────────────────────────────────────────────────────
precmd() { print -Pn "\e]0;%n@%m: %~\a" }

# ── Environment variables (loaded from ~/.config/cas/env) ────────────────────
# Tokens are NOT stored in .zshrc — they live in ~/.config/cas/env
# Format: KEY=VALUE (one per line, # comments allowed)
if [ -f "$HOME/.config/cas/env" ]; then
  set -a
  source "$HOME/.config/cas/env"
  set +a
fi

# ── CAS login ────────────────────────────────────────────────────────────────
alias cas-login='command cas cloud login --token "$CAS_CLOUD_TOKEN"'

# Auto-login on shell start (silent)
command cas cloud login --token "$CAS_CLOUD_TOKEN" &>/dev/null || true
ZSHRC_EOF

chown daniel:daniel "$DANIEL_HOME/.zshrc"

# Create env file template for daniel (tokens filled in manually, not in git)
mkdir -p "$DANIEL_HOME/.config/cas"
if [ ! -f "$DANIEL_HOME/.config/cas/env" ]; then
  cat > "$DANIEL_HOME/.config/cas/env" << 'ENV_EOF'
# CAS environment — tokens and API keys (not committed to git)
GH_TOKEN=YOUR_TOKEN_HERE
GITHUB_TOKEN=YOUR_TOKEN_HERE
CAS_CLOUD_TOKEN=YOUR_TOKEN_HERE
CAS_CLOUD_ENDPOINT=https://petra-stella-cloud.vercel.app
CONTEXT7_API_KEY=YOUR_TOKEN_HERE
NEON_API_KEY=YOUR_TOKEN_HERE
VERCEL_TOKEN=YOUR_TOKEN_HERE
BROSERLESS_API_KEY=YOUR_TOKEN_HERE
ENV_EOF
  chmod 600 "$DANIEL_HOME/.config/cas/env"
fi
chown -R daniel:daniel "$DANIEL_HOME/.config"

# ── 4b. ben's shell environment (.zshrc) ──────────────────────────────────
log "ben's shell environment"

BEN_HOME="/home/ben"

# oh-my-zsh
if [ ! -d "$BEN_HOME/.oh-my-zsh" ]; then
  su - ben -c 'sh -c "$(curl -fsSL https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)" "" --unattended'
fi

# oh-my-zsh plugins
ZSH_CUSTOM_BEN="$BEN_HOME/.oh-my-zsh/custom"
if [ ! -d "$ZSH_CUSTOM_BEN/plugins/zsh-autosuggestions" ]; then
  su - ben -c "git clone https://github.com/zsh-users/zsh-autosuggestions $ZSH_CUSTOM_BEN/plugins/zsh-autosuggestions"
fi
if [ ! -d "$ZSH_CUSTOM_BEN/plugins/zsh-syntax-highlighting" ]; then
  su - ben -c "git clone https://github.com/zsh-users/zsh-syntax-highlighting $ZSH_CUSTOM_BEN/plugins/zsh-syntax-highlighting"
fi

# Write .zshrc (same structure as daniel, placeholder tokens)
cat > "$BEN_HOME/.zshrc" << 'ZSHRC_BEN_EOF'
# ── oh-my-zsh ───────────────────────────────────────────────────────────────
export ZSH="$HOME/.oh-my-zsh"
ZSH_THEME=""  # using starship instead
plugins=(git zsh-autosuggestions zsh-syntax-highlighting)
source "$ZSH/oh-my-zsh.sh"

# ── Starship prompt ─────────────────────────────────────────────────────────
eval "$(starship init zsh)"

# ── eza aliases ──────────────────────────────────────────────────────────────
alias ls='eza --icons --color=always'
alias ll='eza -la --icons --color=always'

# ── CAS wrapper ──────────────────────────────────────────────────────────────
cas() {
  if [ $# -eq 0 ]; then
    command cas factory --new
  else
    command cas "$@"
  fi
}

cas-refresh() {
  echo ">>> Updating CAS..."
  cd ~/projects/cas && git pull && cargo build --release
  sudo cp target/release/cas /usr/local/bin/cas
  echo ">>> CAS login..."
  cas-login
  echo ">>> Cloud auth..."
  command cas cloud auth
  echo ">>> Cloud sync..."
  command cas cloud sync
  echo ">>> Starting factory..."
  command cas factory --new
}

# ── SSH agent persistent socket ──────────────────────────────────────────────
export SSH_AUTH_SOCK="$HOME/.ssh/agent.sock"
if ! ssh-add -l &>/dev/null 2>&1; then
  rm -f "$SSH_AUTH_SOCK"
  eval "$(ssh-agent -a "$SSH_AUTH_SOCK")" >/dev/null 2>&1
  ssh-add ~/.ssh/id_ed25519 2>/dev/null || true
fi

# ── nvm ──────────────────────────────────────────────────────────────────────
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
[ -s "$NVM_DIR/bash_completion" ] && source "$NVM_DIR/bash_completion"

# ── fzf ──────────────────────────────────────────────────────────────────────
[ -f /usr/share/doc/fzf/examples/key-bindings.zsh ] && source /usr/share/doc/fzf/examples/key-bindings.zsh
[ -f /usr/share/doc/fzf/examples/completion.zsh ] && source /usr/share/doc/fzf/examples/completion.zsh

# ── zoxide ───────────────────────────────────────────────────────────────────
eval "$(zoxide init zsh)"

# ── Python ───────────────────────────────────────────────────────────────────
alias python=python3

# ── Terminal title ───────────────────────────────────────────────────────────
precmd() { print -Pn "\e]0;%n@%m: %~\a" }

# ── Environment variables (loaded from ~/.config/cas/env) ────────────────────
if [ -f "$HOME/.config/cas/env" ]; then
  set -a
  source "$HOME/.config/cas/env"
  set +a
fi

# ── CAS login ────────────────────────────────────────────────────────────────
alias cas-login='command cas cloud login --token "$CAS_CLOUD_TOKEN"'

# Auto-login on shell start (silent)
command cas cloud login --token "$CAS_CLOUD_TOKEN" &>/dev/null || true
ZSHRC_BEN_EOF

chown ben:ben "$BEN_HOME/.zshrc"

# Create env file template for ben
mkdir -p "$BEN_HOME/.config/cas"
if [ ! -f "$BEN_HOME/.config/cas/env" ]; then
  cat > "$BEN_HOME/.config/cas/env" << 'ENV_EOF'
# CAS environment — tokens and API keys (not committed to git)
GH_TOKEN=YOUR_TOKEN_HERE
GITHUB_TOKEN=YOUR_TOKEN_HERE
CAS_CLOUD_TOKEN=YOUR_TOKEN_HERE
CAS_CLOUD_ENDPOINT=https://petra-stella-cloud.vercel.app
CONTEXT7_API_KEY=YOUR_TOKEN_HERE
NEON_API_KEY=YOUR_TOKEN_HERE
VERCEL_TOKEN=YOUR_TOKEN_HERE
BROSERLESS_API_KEY=YOUR_TOKEN_HERE
ENV_EOF
  chmod 600 "$BEN_HOME/.config/cas/env"
fi
chown -R ben:ben "$BEN_HOME/.config"

# ── 4c. SSH keypair for ben ───────────────────────────────────────────────
log "SSH keypair for ben"
if [ ! -f "$BEN_HOME/.ssh/id_ed25519" ]; then
  su - ben -c 'ssh-keygen -t ed25519 -C "ben@hetzner-cas" -f ~/.ssh/id_ed25519 -N ""'
  cat "$BEN_HOME/.ssh/id_ed25519.pub" >> "$BEN_HOME/.ssh/authorized_keys"
  chmod 600 "$BEN_HOME/.ssh/authorized_keys"
  chown ben:ben "$BEN_HOME/.ssh/authorized_keys"
fi

# ── 5. Node.js via nvm + pnpm via corepack ─────────────────────────────────
log "Node.js (nvm) + pnpm"

for user in daniel ben; do
  user_home="/home/$user"
  if [ ! -d "$user_home/.nvm" ]; then
    su - "$user" -c "curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/$NVM_VERSION/install.sh | bash"
    su - "$user" -c "source ~/.nvm/nvm.sh && nvm install --lts"
  fi
  su - "$user" -c "source ~/.nvm/nvm.sh && corepack enable && corepack prepare pnpm@latest --activate" 2>/dev/null || true
done

# ── 6. Rust (for all relevant users) ───────────────────────────────────────
log "Rust toolchain"

install_rust_for_user() {
  local username=$1
  local home="/home/$username"
  if [ ! -d "$home/.rustup" ]; then
    su - "$username" -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable"
  else
    su - "$username" -c "source ~/.cargo/env && rustup update stable" 2>/dev/null || true
  fi
}

install_rust_for_user daniel
install_rust_for_user ben
# root rustup (for building CAS as root if needed)
if [ ! -d /root/.rustup ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi

# ── 7. Zig bootstrap (required for CAS ghostty_vt_sys build) ───────────────
log "Zig bootstrap"

if ! command -v zig &>/dev/null; then
  ZIG_VERSION="0.15.2"
  ZIG_ARCHIVE="zig-x86_64-linux-${ZIG_VERSION}"
  ZIG_TAR="${ZIG_ARCHIVE}.tar.xz"
  cd /tmp
  curl -fsSLO "https://ziglang.org/download/${ZIG_VERSION}/${ZIG_TAR}"
  tar xf "$ZIG_TAR"
  rm -rf /opt/zig
  mv "$ZIG_ARCHIVE" /opt/zig
  ln -sf /opt/zig/zig /usr/local/bin/zig
  rm -f "$ZIG_TAR"
fi

# ── 8. CAS — clone, build, symlink ─────────────────────────────────────────
log "CAS binary"

CAS_DIR="$DANIEL_HOME/projects/cas"
if [ ! -d "$CAS_DIR" ]; then
  su - daniel -c "git clone $CAS_REPO ~/projects/cas"
fi

# Build as daniel (has rust toolchain)
su - daniel -c "cd ~/projects/cas && source ~/.cargo/env && cargo build --release"
cp "$CAS_DIR/target/release/cas" /usr/local/bin/cas
chmod 755 /usr/local/bin/cas

# ── 9. Claude Code (global) ────────────────────────────────────────────────
log "Claude Code"

if ! command -v claude &>/dev/null; then
  npm install -g @anthropic-ai/claude-code
fi

# ── 10. Firewall (ufw) ─────────────────────────────────────────────────────
log "Firewall"

ufw --force enable
ufw allow OpenSSH
ufw default deny incoming
ufw default allow outgoing

# ── 11. Swap ────────────────────────────────────────────────────────────────
log "Swap ($SWAP_SIZE)"

if ! swapon --show | grep -q /swapfile; then
  fallocate -l "$SWAP_SIZE" /swapfile
  chmod 600 /swapfile
  mkswap /swapfile
  swapon /swapfile
  grep -q '/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

# ── 12. sysctl tuning ──────────────────────────────────────────────────────
log "sysctl tuning"

cat > /etc/sysctl.d/99-cas.conf << 'SYSCTL_EOF'
vm.swappiness=10
fs.file-max=2097152
net.core.somaxconn=65535
net.ipv4.tcp_max_syn_backlog=65535
SYSCTL_EOF
sysctl --system >/dev/null 2>&1

# ── 13. Disable root password login ────────────────────────────────────────
log "SSH hardening"

sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config
sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
systemctl reload sshd 2>/dev/null || systemctl reload ssh 2>/dev/null || true

# ── 14. Timezone ────────────────────────────────────────────────────────────
timedatectl set-timezone America/New_York 2>/dev/null || true

# ── Done ────────────────────────────────────────────────────────────────────
log "Provisioning complete!"
echo "  Users: daniel (sudo+ssh), ben (sudo+ssh keypair)"
echo "  Shell: zsh + oh-my-zsh + starship + eza + fzf + zoxide"
echo "  Node:  $(su - daniel -c 'source ~/.nvm/nvm.sh && node --version' 2>/dev/null || echo 'check manually')"
echo "  Rust:  $(su - daniel -c 'source ~/.cargo/env && rustc --version' 2>/dev/null || echo 'check manually')"
echo "  CAS:   $(cas --version 2>/dev/null || echo 'check manually')"
echo "  Claude: $(claude --version 2>/dev/null || echo 'check manually')"
echo "  UFW:   $(ufw status | head -1)"
echo "  Swap:  $(swapon --show --noheadings | awk '{print $3}')"
