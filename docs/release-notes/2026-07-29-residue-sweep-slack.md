# Slack drafts — v2.35.0 (2026-07-29)

Channel: #cas-internal (C0B44GUKDK2)
Two distinct TOP-LEVEL posts. Not threaded. Post after the tag is pushed.

STATUS: POSTING APPROVED by operator 2026-07-29 (standing authorization for this wave).

---

## POST 1 — User perspective (top-level)

**Was:** Grok-based workers were second-class citizens — they looked stalled while working, captured no history of what they committed, and checking on any worker got slower the longer the day ran.
**Now:** every supported coding agent gets the same accurate status, the same commit tracking, and fast health checks all day.

- A Grok worker that is busy now reads as busy. Its activity was previously invisible to the status view, so healthy workers looked dead — the same illusion fixed for Codex last release, now fixed everywhere.
- Grok sessions now record which commits belong to which task, both inside and outside factory runs. Previously that bookkeeping silently did nothing for Grok, which later forced manual overrides at completion checks.
- Checking worker status no longer rescans the entire session history every time — health checks stay fast no matter how long the machine has been running.
- Work completion checks got stricter where it matters and looser where it hurt: a commit made after moving into another project's directory is no longer credited to the wrong project, while ordinary "change directory, then commit" habits work again without penalty.
- A completion check can no longer quietly pass when real work is sitting unmerged behind a broken reference — it now falls back to checking the actual branches, including after a task changed hands between workers.
- An alert about an idle worker can no longer be permanently silenced by one stuck message retry.

---

## POST 2 — Dev perspective (top-level)

**Was:** the Grok harness diverged from the shared resolution paths, commit anchoring guessed from command text, and two gate checks trusted references that could be stale or nonexistent.
**Now:** one harness-aware resolver, semantic anchor validation, and existence-verified refs with branch fallbacks.

- **Grok parity:** worker_status/worker_activity route Grok through the same harness-aware transcript resolver as the wedged check, sharing the TTL-amortized cache added for Codex; hook payloads normalize Grok's camelCase fields and terminal tool name into the standard shape (env-based detection in factories, payload-shape detection standalone), so anchors and AI attribution now record for Grok sessions.
- **Anchor integrity:** commit anchoring stopped parsing command text to guess redirection (a scheme that both rejected legitimate `cd && git commit` shapes and missed GIT_DIR/env and case variants). The extracted commit hash is now resolved against the hook cwd's own repository — if it resolves there, it anchors; if not, it doesn't. Amend/reset behavior preserved.
- **Gate correctness:** ref-existence checks now verify the object exists (`cat-file -e`), so a dangling anchor SHA falls back to branch inspection instead of degrading the unmerged count to zero; the fallback also evaluates the current assignee's live branch, not just the first assignee's historical parked branch, closing a reassignment blind spot. A parked branch pointer is never overwritten by a later worker's commit.
- **Queue/status semantics:** scheduled-retry prompts count as pending for idle-suppression observability without re-latching alerts forever (bounded, target-fair eligibility restored); empty target universes are a loud error instead of a silent no-op; bounded rollout tail reads recover from UTF-8 boundary splits; lease release reasons live only in their canonical column.
- **Test infrastructure:** nested env-guards fail loudly instead of deadlocking; seven previously-untested protective guard branches are pinned; reused-cwd freshest-CLI rollout selection is pinned.
