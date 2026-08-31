# Release notes — knowledge-loop hardening + sync fixes (merged to main 2026-08-29/30)

> Target: `#cas-internal` (`C0B44GUKDK2`) · Label: **Live on production**
> Embargo lifted 2026-08-31; posted (see POSTED).
> Covers PRs #615–#623.

---

## User thread

**Top-level:**
Live on production · **User** · Cassy now has to *prove* a rule works before it starts injecting it into your sessions — and every rule and skill keeps full version history you can roll back.

**Threaded reply:**
- **Rule promotion** — Was: a single "helpful" report promoted a rule straight into every future session. Now: promotion requires repeated, independent evidence across multiple sessions, and rules that keep getting corrected or flagged harmful are automatically demoted back out.
- **Version history & undo** — Was: editing or deleting a rule or skill destroyed the old version permanently. Now: every change is recorded with who/when/why, deletes are reversible tombstones, and any prior version can be restored.
- **Skill safety checks** — Was: a skill's validation script was stored but never run. Now: it runs in a network-isolated sandbox before the skill is accepted; a failing check blocks the change and keeps the previous version live.
- **Where knowledge comes from** — Was: merged or promoted knowledge lost all links to its sources. Now: rules, skills, and learnings carry their ancestry end-to-end, visible in `show`.
- **Usage tracking** — Was: "how often was this rule actually used?" had no real answer. Now: every injection is recorded and an impact report joins usage to session outcomes.
- **Old activity data** — Was: events older than 30 days were deleted forever. Now: they age into a compressed, size-capped archive you can query.
- **Sync reliability** — Was: some task deletions and moved tasks silently never reached the cloud (server said OK but skipped them). Now: deletions and moves carry full project identity, silent skips surface as errors, and the stuck backlog was flushed and verified.

---

## Dev thread

**Top-level:**
Live on production · **Dev** · Rule/skill lifecycle is now gated on measured `retrieval_outcomes` evidence with versioned, tombstoned mutations — plus wire-scope fixes that end the silent-skip/400 family on team and personal sync (PRs #615–#623).

**Threaded reply:**
- **Promotion gating (#622)** — Was: `rule helpful` flipped Draft→Proven in one call; `harmful` never demoted. Now: promotion needs a configurable multi-signal threshold (helpful floor ≥2 AND retrieval evidence from ≥2 distinct sessions, usefulness ≥0.5, zero corrections/harmful); threshold-floored demotion lands through `update_with_metadata` with a version-ledger change note. `RetrievalAggregate` gains a privacy-preserving `distinct_sessions` count.
- **Version ledger (#617, m242)** — Was: rule/skill mutations were destructive in-place UPDATEs; "archive" was a hard DELETE. Now: every create/update/delete/restore writes a version row with a lifecycle `operation` column; deletes are `Retired` tombstones; restore is a supported action.
- **Validation gate (#620)** — Was: `Skill.validation_script`/pre/postconditions were three dormant columns. Now: scripts execute pre-persist in a bubblewrap sandbox on Linux (`--unshare-net`, ro-binds, tmpfs, cleared env) with plain-sh fallback + explicit warning when bwrap is absent; `skill_validation.require_sandbox` enables fail-closed; rejected updates leave the prior version and its history untouched.
- **Provenance (#623)** — Was: consolidation and promotion destroyed source links. Now: `merge_source_ids` unions ancestry (ordered, deduped) through consolidation; session-stop synthesis stamps `source_ids` onto learnings/rules; SKILL.md sync round-trips them; `show` surfaces the chain.
- **Impact tracking (#619, m243)** — Was: `rules.surface_count` was schema-only. Now: injection increments it via a batched callback and records per-session `surfaced_artifacts` rows joined to session outcomes in a new impact report (ledger write ~9ms, sub-ms per artifact).
- **Trace archive (#618)** — Was: daemon maintenance hard-deleted events/recordings at 30 days. Now: archive-before-delete into zstd JSONL under `.cas/archive/`, 1 GiB configurable cap with oldest-first eviction and logged evictions, range list/sample read APIs.
- **Sync wire scope (#615, #616, #621)** — Was: team task upserts without explicit `scope` were silently skipped (HTTP 200, `skipped:N`); team/personal deletes without `project_id` got 400s and parked forever; a dangling-symlink `cloud.json` could route a test fixture write into a real project. Now: upserts stamp `scope=project`, both delete paths carry `project_id` (slash-encoded), all-skipped 200s surface client-side as failures, the symlink guard rejects symlink destinations, and the parked backlog was flushed with server-verified deletion and non-resurrection.

## POSTED

Channel: `#cas-internal` (`C0B44GUKDK2`) · Posted 2026-08-31 via the approved Claude profile route (embargo lifted by the operator on 2026-08-31).

| Message | Slack ts | Permalink |
| --- | --- | --- |
| User top-level | 1788180456.703989 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180456703989 |
| User reply (Was → Now) | 1788180464.279309 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180464279309?thread_ts=1788180456.703989&cid=C0B44GUKDK2 |
| Dev top-level | 1788180457.431129 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180457431129 |
| Dev reply (Was → Now) | 1788180470.446359 | https://petra-stella.slack.com/archives/C0B44GUKDK2/p1788180470446359?thread_ts=1788180457.431129&cid=C0B44GUKDK2 |
