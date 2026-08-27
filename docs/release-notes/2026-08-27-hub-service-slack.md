# 2026-08-27 — Commander hub as a managed service — #cas-internal

> EMBARGO: do not post before 2026-08-31 (operator confirmation required).
> Draft complete; append the POSTED receipt table after publication.

## User thread

**Top-level (Live on production · User):**

🖥️ The Commander hub now installs as an always-on service that starts with
your machine and restarts itself if it crashes — no more remembering to launch
it by hand.

**Reply (Was → Now):**

Was: the hub behind Commander was a manually-launched process. If you rebooted
or it crashed, Commander stayed dark until someone remembered to start it
again.

Now: one command installs it as a proper background service — on Linux as a
per-user systemd service (including the setting that lets it start on boot
before you log in), on a Mac as a launchd agent. It starts immediately, comes
back after reboots and failures, and writes its logs to a predictable place.
The same command family answers honestly whether the service is installed,
enabled, and running, can preview what it would do without touching anything,
and uninstalls cleanly. The installer's "next steps" now point new machines at
it from minute one.

## Dev thread

**Top-level (Live on production · Dev):**

⚙️ New `cas hub service install|uninstall|status` (with `--dry-run`): writes +
enables a systemd user unit on Linux (linger handled) and a launchd
LaunchAgent on macOS, restart-on-failure, no secrets in unit files (PR #593).

**Reply (Was → Now):**

Was: `cas hub serve` had to be run by hand; nothing survived a reboot, and
service supervision was ad hoc per machine.

Now: `cas hub service install` generates and enables the platform service —
Linux: `~/.config/systemd/user/cas-hub.service` via `systemctl --user enable
--now` with `loginctl enable-linger` handled or loudly instructed; macOS: a
LaunchAgent bootstrapped with `launchctl bootstrap gui/$UID` plus kickstart.
Units restart on failure, log to `~/.cas/hub/hub.log`, and carry no secrets
(credential stores are referenced, never inlined). `status` reports
installed/enabled/active as JSON-backed truth, re-runs are idempotent, and
`--dry-run` prints the exact unit/plist and commands. Generation seams are
covered by fixture tests on both platforms; a live enabled+active receipt was
captured on a Linux host. The install script's next steps and the
commander-fleet runbook now include the service path.

## POSTED

(to be filled at publication — parent/reply permalinks + timestamps for both threads)
