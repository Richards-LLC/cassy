#!/usr/bin/env bash
# Install the self-hosted Mac release runner (cas-195e) as a launchd user agent.
# release-prebuild.yml routes trusted macOS prebuilds to its cas-mac-release
# label; the persistent CARGO_TARGET_DIR keeps release builds incremental.
# Run as the login user, never root, on an Apple Silicon Mac with Xcode 26.3:
#   scripts/install-mac-release-runner.sh              # install or repair
#   scripts/install-mac-release-runner.sh --uninstall  # deregister and remove
# Registration uses the caller's gh (repository admin) once; jobs run with a
# private HOME, so they never see the caller's git, gh or ssh configuration.
set -euo pipefail

repo=Richards-LLC/cassy
label=dev.cassy.mac-release-runner
root="${CAS_MAC_RELEASE_ROOT:-$HOME/.cassy-mac-release}"
runner_dir="$root/runner"
plist="$HOME/Library/LaunchAgents/$label.plist"
domain="gui/$(id -u)"
version=2.338.2
archive="actions-runner-osx-arm64-$version.tar.gz"
sha256=25a065af409ffa317ab4d5a3b614dd15fc9c40f2172bc1b35a1b1258e919a388

fail() { echo "error: $1" >&2; exit 1; }
[[ "$(id -u)" -ne 0 ]] || fail "run as the login user, not root"
[[ "$(uname -s)/$(uname -m)" == Darwin/arm64 ]] || fail "needs an Apple Silicon Mac"

if [[ "${1:-}" == --uninstall ]]; then
    launchctl bootout "$domain/$label" 2>/dev/null || true
    rm -f "$plist"
    if [[ -f "$runner_dir/.runner" ]]; then
        token="$(gh api -X POST "repos/$repo/actions/runners/remove-token" --jq .token)"
        (cd "$runner_dir" && ./config.sh remove --token "$token")
    fi
    rm -rf "$root"
    echo "removed the Mac release runner and $root"
    exit 0
fi

[[ -d /Applications/Xcode-26.3.0.app ]] || fail "release-prebuild.yml expects /Applications/Xcode-26.3.0.app"
mkdir -p "$runner_dir" "$root/home" "$root/cargo-target" "$root/logs" "$(dirname "$plist")"

if [[ ! -x "$runner_dir/config.sh" ]]; then
    curl -fsSL -o "$root/$archive" "https://github.com/actions/runner/releases/download/v$version/$archive"
    echo "$sha256  $root/$archive" | shasum -a 256 -c -
    tar -xzf "$root/$archive" -C "$runner_dir"
    rm "$root/$archive"
fi

launchctl bootout "$domain/$label" 2>/dev/null || true
if [[ ! -f "$runner_dir/.runner" ]]; then
    token="$(gh api -X POST "repos/$repo/actions/runners/registration-token" --jq .token)"
    (cd "$runner_dir" && ./config.sh --unattended --replace --url "https://github.com/$repo" --token "$token" \
        --name "$(scutil --get LocalHostName)-cas-mac-release" --labels cas-mac-release --work "$root/work")
fi

# Jobs get a fixed PATH and the private HOME, toolchain and target directory.
printf '%s\n' /opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin > "$runner_dir/.path"
printf 'LANG=en_US.UTF-8\nHOME=%s\nCARGO_TARGET_DIR=%s\n' "$root/home" "$root/cargo-target" > "$runner_dir/.env"
cp "$runner_dir/bin/runsvc.sh" "$runner_dir/runsvc.sh"

cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key><array><string>$runner_dir/runsvc.sh</string></array>
  <key>WorkingDirectory</key><string>$runner_dir</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>ACTIONS_RUNNER_SVC</key><string>1</string>
    <key>HOME</key><string>$root/home</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>SessionCreate</key><true/>
  <key>StandardOutPath</key><string>$root/logs/runner.log</string>
  <key>StandardErrorPath</key><string>$root/logs/runner.log</string>
</dict>
</plist>
PLIST
plutil -lint "$plist" > /dev/null
launchctl enable "$domain/$label"
launchctl bootstrap "$domain" "$plist"
echo "installed $label; check: gh api repos/$repo/actions/runners --jq '.runners[] | [.name, .status] | @tsv'"
