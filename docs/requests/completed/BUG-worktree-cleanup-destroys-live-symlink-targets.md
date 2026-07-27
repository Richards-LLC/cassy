# BUG: Worktree cleanup destroyed files still referenced by live $HOME symlinks

**Date:** 2026-07-27
**Severity:** High (user-facing desktop breakage, discovered after reboot)
**Repo:** soundwave-config (factory/theme-dolphin → epic cas-cd8b)

## What happened

During the Tokyo Night Storm epic (2026-07-24), a factory worker ran the
dotfile stow/install step from **inside its isolated worktree**
(`~/soundwave-config/.cas/worktrees/theme-dolphin/`). It replaced ~21 live
symlinks in `$HOME` — including `.gitconfig`, `.ssh/config`, `.vimrc`,
`.bash_profile`, `~/bin/vramp.py`, `~/bin/piploom`, systemd user units
(`linux-whispr.service`, krdpserver override, inhibit-suspend), Konsole
profile, color-scheme, icon theme, starship, secrets.bash — with symlinks
pointing **into the worktree** instead of the main checkout.

The worktree was later removed (merge/cleanup/GC), leaving all those
symlinks dangling. Nothing visibly broke until the 2026-07-27 reboot, when
every app re-read config: theme gone, tray/dock icons broken, krdpserver
failed, linux-whispr "not-found", git identity + SSH config silently gone.

Additionally, the epic branch was never merged to master, so the worktree
was the ONLY on-disk copy of some files (Konsole TN profile, TN color
scheme, TN icon theme) — recovered today by merging
`epic/epic-tokyo-night-storm-desktop-finish-soundwave-cas-cd8b` → master.

## Asks

1. **Worktree cleanup guard:** before removing a worktree, scan for (or
   maintain a registry of) external symlinks pointing into it; refuse or
   warn loudly if any exist.
2. **Worker policy:** stow/install steps that create symlinks into the repo
   MUST target the main checkout path, never `$REPO/.cas/worktrees/...`.
   Consider a lint in the worker skill.
3. **Stale worker guard hook:** `cas factory` leaves its pre-commit guard
   ("Workers may only commit on factory/<name>") installed in the MAIN
   repo's `.git/hooks/pre-commit`, blocking the owner's and supervisor's
   legitimate commits on master. Guard should be scoped to worker worktrees
   or removed on factory shutdown.

## Recovery performed (2026-07-27, supervisor tender-lark-48)

- Merged epic cas-cd8b → master (2 trivial append conflicts in DECISIONS.md), pushed to unicron.
- Repointed all 21 dangling symlinks from `.cas/worktrees/theme-dolphin/<path>` → `~/soundwave-config/<path>`.
- daemon-reload; restarted linux-whispr, inhibit-suspend, krdpserver; relaunched vramp; kbuildsycoca6 + plasmashell --replace.
- Outstanding: monitor settings (kwinoutputconfig.json had no backup — user re-set manually), ghostty install unaccounted for, `/opt/appimagelauncher.AppDir` missing (appimagelauncherd.service dangling).

---

## Resolution

**Resolved 2026-07-27 — shipped in cas v2.29.0 (`origin/main`).** Task: cas-df97

Worktree removal now blocks on untracked content (removal destroys it) and guards external symlink targets — see `cas-cli/src/worktree/external_symlinks.rs`. Merge-only still warns rather than blocking.
