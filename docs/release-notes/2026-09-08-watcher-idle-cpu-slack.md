# Slack draft — code watcher idle CPU fix (main merge, PR #765)

Channel: #cas-internal. Deploy target: Live on production (main). Reaches installed hosts with the next runtime release.

## User thread

Top-level:
Live on production · User · Cassy's background server no longer burns a CPU core while it sits idle.

Reply:
Was → every Cassy server under a large project quietly used most of a core doing nothing, which made the machine feel busy and slowed real builds. Now → an idle server uses under two percent, and the fix is guarded by a test that measures it.

## Dev thread

Top-level:
Live on production · Dev · The daemon code watcher drops access events and ignored paths at the raw notify callback and replaces notify-debouncer-mini with a bounded in-tree debounce loop.

Reply:
Was → notify-debouncer-mini's loop re-armed a past deadline forever, and the recursive watch reached vendor/ghostty through a symlink under crates/, so every `cas serve` in cas-src spent 65-70% of a core in the debouncer thread. Now → `EventKind::Access` and ignored paths are filtered before debouncing, symlinks are not followed, the debounce loop waits on the earliest pending deadline with a 1 ms floor and stops on drop; a Linux regression asserts under 2% CPU over 5 s with eight concurrent writers (red on the old code at 222 ticks). PR #765, closes #754.

## POSTED
Posted 2026-09-08 22:03Z via the claude.ai Slack MCP to #cas-internal (C0B44GUKDK2):
- User top-level ts 1788904997.102129 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788904997102129 (reply in thread)
- Dev top-level ts 1788904998.110369 https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788904998110369 (reply in thread)
