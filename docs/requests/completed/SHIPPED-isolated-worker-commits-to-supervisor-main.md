---
from: gabber-studio factory team
date: 2026-06-07
priority: P1
cas_task: cas-e5a1
---

# Isolated worker (`isolate=true`) commits land on the supervisor's `main` checkout instead of its own worktree

During a factory run, a worker spawned with `mcp__cas__coordination action=spawn_workers isolate=true` committed its task work **directly onto the supervisor's `main` branch in the shared repo checkout**, not onto its own `factory/<name>` branch inside its assigned worktree. A second worker spawned in the *same* call behaved correctly. The result is a split-brain where un-reviewed feature code — including a Prisma migration and a change to a live outbound-SMS chokepoint — accumulated **unpushed on `main`**, one `git push` away from auto-deploying a schema the prod DB doesn't have.

**This is a repeat occurrence.** The gabber-studio team has a standing memory (`feedback_workers_commit_direct_to_main`) from a prior incident describing the same failure mode. Filing now because it keeps happening and the blast radius is prod-deploy-shaped.

## Affected version

`cas 2.19.0 (78656f4-dirty 2026-06-02)`. Observed 2026-06-07 in repo `gabber-studio` (push-to-`main` triggers the backend prod deploy via `main-build.yml`).

## Symptom

Two workers were spawned in one session, both `isolate=true`:

- `frontend` → worktree `.cas/worktrees/frontend`, branch `factory/frontend`
- `backend`  → worktree `.cas/worktrees/backend`, branch `factory/backend`

The **frontend** worker behaved correctly: it committed on `factory/frontend` inside its worktree and opened PRs **#1095/#1096** into the epic branch. Textbook.

The **backend** worker's two task commits (`4c6add3c`, `7659a0e5`) landed on **`main` in the supervisor's primary checkout** (`/home/pippenz/Petrastella/gabber-studio`), leaving local `main` 2 commits ahead of `origin/main` and **unpushed**. Its *own* worktree branch `factory/backend` was still sitting at the base commit (`21acafb9`) with zero commits.

Directly observed topology at discovery time:

```
$ git worktree list
/home/pippenz/Petrastella/gabber-studio                          7659a0e5 [main]            # supervisor checkout — has the worker's commits
/home/pippenz/Petrastella/gabber-studio/.cas/worktrees/backend   21acafb9 [factory/backend]  # worker's OWN worktree — EMPTY, still at base
/home/pippenz/Petrastella/gabber-studio/.cas/worktrees/frontend  3803d1dd [factory/frontend] # other worker — correct

$ git branch -r --contains 4c6add3c     # nothing — not on origin
$ git branch    --contains 4c6add3c     # main only — not on the epic branch, not on factory/backend
```

So the same spawn call produced one worker that operated inside its worktree and one that operated against the shared `main` checkout. The asymmetry points at nondeterministic worktree/cwd setup, not a hard "isolation is off" config.

A possibly-related inconsistency: `mcp__cas__coordination action=worktree_list` returns *"Worktrees are experimental and disabled by default"* even though `spawn_workers isolate=true` clearly **did** create worktrees (`worker_status` reports `Clone: .cas/worktrees/backend`, and `git worktree list` shows them). The status surface and the actual spawn behavior disagree — if isolation silently degrades for some workers, it does so without signalling.

## Impact

1. **Split-brain across branches.** Backend lanes ended up on `main`; frontend lanes on the epic branch. Neither branch had the full feature; the supervisor had to manually cherry-pick the backend commits onto the epic branch and `git reset --hard origin/main` to recover.
2. **Prod-deploy footgun.** In this repo, push-to-`main` triggers the backend deploy. The orphaned commits included a Prisma migration (`20260607120000_add_notifications`) plus a hook in the `SmsSenderService.sendSms` chokepoint that reads/writes the new table. The migration is **not** applied to any DB yet (Vercel doesn't run `prisma migrate` here). A reflexive `git push` of local `main` would have deployed code that throws on **every outbound SMS** against a missing table. Recovery depended entirely on the commits being unpushed — luck, not design.
3. **Review-gate bypass.** Work that should flow through `factory/<name>` → PR → epic-merge review instead sat directly on the release branch, skipping the supervisor's persona-review gate.
4. **Recurring.** See `feedback_workers_commit_direct_to_main` — same class of failure previously, hence this report.

No data was lost in this instance (commits were recoverable), but that's the only thing that went right.

## Root cause — hypothesis (source not yet pinpointed)

Not verified against `cas-cli` source, so stated as a hypothesis to investigate:

The worker's **process working directory was not switched into its worktree** before the agent began running `git`. The team/session `cwd` for this run is the primary repo root (`config.json` lists `cwd: /home/pippenz/Petrastella/gabber-studio` for members). If a worker's launched process inherits that session `cwd` and the worktree-`cd` step races or is skipped, every `git commit` the worker runs targets the **primary checkout's current branch (`main`)** — exactly the observed result. The fact that one of two identically-spawned workers got it right suggests a race or ordering bug in the per-worker worktree/cwd setup rather than a global config toggle.

Suggested places to look on the CLI side: the worker launch path that creates the worktree and starts the tmux pane / agent process — confirm it (a) creates the worktree, (b) sets the spawned process `cwd` to the worktree path, and (c) blocks the agent from starting until both are done.

## Suggested fix / guards (defense in depth)

1. **Set and verify the worker process `cwd` = worktree path.** Launch the worker process with `current_dir(worktree_path)` explicitly; have the worker preamble run `pwd` / `git rev-parse --show-toplevel` and assert it equals the assigned worktree before doing any work. Abort loudly on mismatch.
2. **Protected-branch commit guard in worker sessions.** Hard-block `git commit` (and merge) when `HEAD` resolves to `main`/`staging`/`master` in a worker context. Workers should only commit on `factory/<name>` or `worker/<task>` branches. A `pre-commit`-style guard installed into the worker's repo/worktree, or a wrapper in the worker's git invocation path, that refuses with a clear message.
3. **Reconcile the worktree feature-flag surface.** Make `worktree_list` / `worktree_status` reflect actual state, and make `spawn_workers isolate=true` **fail loudly** if it cannot establish isolation instead of silently falling back to the shared checkout.
4. **Post-spawn assertion.** After spawning, the daemon verifies each isolated worker's branch is `factory/<name>` and its `HEAD` is detached from the protected branch; surface a warning to the supervisor if not.

## Acceptance criteria

1. A worker spawned with `isolate=true` runs all `git` operations inside its assigned worktree; `git rev-parse --show-toplevel` in the worker session returns the worktree path, never the primary checkout.
2. No worker can create a commit whose `HEAD` is `main`/`staging` — the attempt is blocked with an actionable error.
3. Spawning N isolated workers in a single call yields N workers all correctly scoped to their worktrees (no per-worker nondeterminism); a stress spawn of ≥4 reproduces zero leaks to `main`.
4. `worktree_list`/`worktree_status` accurately report whether isolation is active; `isolate=true` either guarantees isolation or errors — never silently degrades.
5. Regression: the correct path (worker commits on `factory/<name>`, PRs into the epic branch) is unchanged.

## Demo statement (Definition of Done)

Spawn 4 isolated workers and assign each a task that makes a commit. After all four close, `git log main` shows **zero** worker commits, `origin/main` is untouched, and each `factory/<name>` branch carries exactly its worker's commits. Any worker that attempts to commit on `main` is stopped before the commit object is created, with a message telling it to switch to its worktree branch.

## References

- Observed topology: `git worktree list`, `git branch --contains <sha>` (this run, repo `gabber-studio`).
- Prior incidence: gabber-studio memory `feedback_workers_commit_direct_to_main`.
- Recovery performed by supervisor: cherry-pick `4c6add3c 7659a0e5` onto the epic branch (→ `2c32f9df`, `39c6ac8a`), then `git reset --hard origin/main` to defuse the unpushed-main risk.
- Deploy trigger context: gabber-studio backend deploys on push to `main`/`staging` (`main-build.yml`); Vercel does **not** run `prisma migrate`, so schema-dependent code on `main` is doubly dangerous.

---

## Update — second incident same day, and the bigger problem: the supervisor is blind to worker state (2026-06-07)

The leak recurred a third+ time in the same session with a different (frontend) worker — but the more important finding is the **compounding failure it exposed: the supervisor has no reliable view of what a worker is actually doing or where its commits land.** The human watching the worker's terminal could see the truth; the supervisor could not, and burned ~8 tool calls on git forensics to reconstruct it.

### What happened

1. Frontend worker was assigned two tasks. It did the work but committed it onto the **supervisor's shared checkout** (`epic` branch in the primary working dir), not its `factory/frontend` worktree — the same leak as above. (The worker itself later narrated: *"my earlier commits went to the main repo on the epic branch (wrong place)."*)
2. The worker then **closed the Cassy task** (status → `Closed` via the dual-gate workaround). So from the task DB, the supervisor saw "done."
3. But: there was **no PR**, the commits were **not on `origin/epic`**, and the supervisor's own `git status` / `git log` were now polluted with commits it never made (`7adca583` etc. sitting on the supervisor's local `epic`, unpushed).
4. The worker eventually self-corrected (cherry-picked the stray commits into its `factory/frontend` worktree, force-pushed, opened a PR) — but the supervisor only learned this by reading the worker's terminal scrollback, which was surfaced **by the human**, not by any tool.
5. Separately, the worker discovered the filed task pointed at the wrong file (`p/[slug]/settings.vue` had no overlap; the real bug was `brands/[id].vue`) — a correct call the supervisor had no way to see being made.

### Why this is worse than the base leak

- **Task status lies.** `Closed` (+ dual-gate workaround) masks "committed to the wrong ref, never merged, no PR." A supervisor that trusts task status ships nothing or ships stale.
- **No worker-state introspection.** There is no `coordination` action that answers "what has worker X committed, to which branch, is it pushed, is there a PR?" The supervisor resorts to `git cat-file` / `git log <ref>..<ref>` / `ls` archaeology — on a working tree the worker may be concurrently mutating (I hit `git checkout` aborts because the worker had uncommitted changes in *my* tree).
- **Shared checkout corrupts the supervisor's own git.** Recovery required `git reset --hard origin/epic` on the supervisor's checkout to discard the worker's stray commits — i.e., the bug doesn't just leak worker work, it makes the *supervisor's* repo state untrustworthy.

### Suggested additions to the fix

- **Supervisor-facing worker git introspection:** a `coordination action=worker_status` that reports per worker — current branch, worktree path, HEAD sha, ahead/behind vs base, dirty/clean, **last pushed ref + open PR URL**. "Done" must be verifiable without git forensics.
- **Gate `task close` on merge reality:** refuse close (or mark `pending-merge`) when no commit is reachable from the worker's `factory/<name>` branch / no PR exists for the change. Kill the dual-gate "Closed but unmerged" path.
- **Hard-isolate the worker's git from the supervisor's checkout** (the core fix) so neither can commit into or corrupt the other's working tree / branch refs. The supervisor should never see worker commits appear on its own `HEAD`.
- **Surface worker reasoning to the supervisor, not just the human.** The worker made several correct decisions (wrong-file detection, self-correcting the branch) that were invisible to the orchestrator — the orchestrator needs a feed of worker progress/decisions to coordinate, instead of relying on the human to relay the terminal.
