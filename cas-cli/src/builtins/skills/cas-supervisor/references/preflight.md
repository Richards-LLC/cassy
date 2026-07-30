# Pre-flight: is the running `cas serve` binary current? (cas-d0f9)

**Read this before you spawn workers.** The running `cas serve` daemon is whatever binary was installed the last time `cargo build --release` ran in this repo. If the binary predates recent close-gate / verification / hook changes, workers may see legacy global verification blocks instead of the current exact-task close gate.

The canonical instance of this was 2026-04-22 (~8 min × 2 closes wasted) when `cas serve` predated commit `bba6fbf` (factory worker verification jail exemption). That fix is on `main` and has been for a while — but stale deploys keep re-discovering it.

**30-second check before `mcp__cas__coordination action=spawn_workers`:**

`cas --version` prints `cas <semver> (<hash>[-dirty] <date>)`, e.g. `cas 2.27.0 (9b52e17-dirty 2026-07-16)`. Extract the short hash after `(`; strip optional `-dirty` (means the binary was built with a dirty worktree — not part of the git object id). **Do not** use `awk '{print $NF}'` — that yields the date token (`2026-07-16)`), not the hash.

```
# extract short hash (sample → 9b52e17)
cas_hash=$(cas --version | sed -E 's/.*\(([0-9a-f]+)(-dirty)? .*/\1/')
git rev-parse HEAD                                 # repo HEAD
git log --oneline HEAD --not "$cas_hash" -- \
    cas-cli/src/mcp cas-cli/src/hooks cas-cli/src/cli/factory 2>/dev/null | head -20
```

If the third command returns any commits touching `close_ops`, `verify_ops`, `pre_tool`, hooks, or factory orchestration, **rebuild before proceeding**:

```
cargo build --release
# restart any running `cas serve` processes so they pick up the new binary
```

If you don't rebuild and close-time jails every task, that's on you. The running-cas-vs-HEAD drift is the single highest-cost preventable session friction (epic cas-9508).

Shortcut: `cas --version | sed -E 's/.*\(([0-9a-f]+)(-dirty)? .*/\1/'` vs `git rev-parse --short HEAD`. If they match, skip the log check and proceed.
