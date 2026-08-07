# BUG — Cross-project task replication: residual contamination + two live re-entry paths

**Filed:** 2026-08-06
**Filed from:** gabber-studio (`/home/pippenz/Petrastella/gabber-studio`)
**cas version observed:** `cas 2.45.0 (5f6fd29-dirty 2026-08-06)`
**Severity:** P1 — 1,600+ foreign task rows still resident in every project DB on this
machine; two mechanisms can still produce *new* contamination today.
**Related (already fixed):** `cas-ed15` / EPIC `cas-2eb3` (v2.15.0, 2026-05-12),
`cas-53d5` (v2.15.2), `cas-1ced` (v2.15.3)

---

## 1. Symptom

`mcp__cas__task action=ready` run from **gabber-studio** returned 218 non-closed tasks,
of which **66 belonged to five other projects**: Roark Realty tax work (Accounting),
payment-reminder / recurring-invoice work (abundant-mines), OpenClaw-on-Vultr work
(ozer), telehealth wallet/session-credit work (ozer), and cas-cli factory work
(cas-src). Closed foreign replicas raised the count to **125 rows** attributable by
epic-child expansion alone.

Daniel's report: *"other projects' tasks/epics are polluting gabber-studio's CAS queue."*

## 2. Confirmed root cause (already fixed upstream — this section is corroboration)

The v2.15.0 CHANGELOG entry for `cas-ed15` describes exactly what happened:

> `cas cloud pull` … previously built its URL inline via raw `ureq::get` and never
> appended `project_id=`, bypassing the scoped `CloudSyncer::pull` abstraction…
> The leak returned `team_id IS NULL` rows from all of a user's projects on every
> pull, contaminating local DBs with foreign-project data.

Corroborating evidence measured on this machine:

| Evidence | Value |
|---|---|
| `team_id IS NULL` rows in gabber `tasks` (pre-surgery) | **2,551 / 2,896** — matches the "`team_id IS NULL` rows from all projects" leak shape |
| Every `.cas/cloud.json` on the machine | same `endpoint` + same personal token `psc_k1_…8056` → one personal scope, all projects |
| Newest foreign task created in gabber | `2026-05-04` (`cas-e9e9` family) |
| gabber `.cas/cloud.json` `last_task_sync` | `2026-05-06T12:37:46Z` |
| `cas-ed15` fix shipped | **v2.15.0, 2026-05-12** |
| Task-row overlap gabber ↔ each other project DB | ~1,580–1,820 rows each (6 DBs sampled) |
| Fingerprint of a broadcast, not independent work | the 16-task abundant-mines "payment reminders" set appears in **10 different DBs** with *byte-identical* `SUM(LENGTH(notes)) = 756` and `12 open / 4 closed` in nine of them |

Contamination stopped before the fix date and has not resumed. **The pull leak itself is
fixed.** What follows is what is *not*.

## 3. What is still broken

### 3.1 Residual contamination is never cleaned up (P1)

`cas cloud purge-foreign` exists (added v2.0.0) but nothing ever runs it, and nothing
tells a user their DB is contaminated. On this machine, ~1,700 foreign rows per project
DB remain — invisible in `ready` only because most are `closed`. Any project that
re-opens or re-lists history sees another project's work.

**Ask:** a one-shot, non-destructive `cas doctor` check that reports foreign-row counts
per project DB, plus guidance to run `purge-foreign`. Ideally run automatically on
version upgrade past 2.15.0 for DBs whose rows predate the fix.

### 3.2 `canonical_id` collides on folder name — LIVE, will contaminate today (P0-ish)

`resolve_canonical_id()` falls back to the **parent folder name** when
`.cas/config.toml [project] canonical_id` is unset. **Not one project on this machine
pins it** — verified by grep across gabber-studio, ozer, cas-src, abundant-mines,
domdms and both Accounting checkouts.

Two *distinct* projects therefore resolve to the same cloud bucket:

```
/home/pippenz/Petra Stella/Accounting   -> canonical_id "Accounting"
/home/pippenz/Richards LLC/Accounting   -> canonical_id "Accounting"
```

These are different clients' books. Project-scoped pull does not help when two projects
claim the same scope — they will keep merging into each other on every sync.

**Ask:** prefer `derive_canonical_id_from_git_remote()` over the folder-name fallback
(the function already exists and is used in `resolve_canonical_id`'s *config-write*
path but not in the read chain), and/or warn loudly when two `.cas` roots on the same
machine resolve to the same `canonical_id`.

### 3.3 Tasks started and closed from the wrong project root (P2)

Independent of cloud sync, agents have run task lifecycle against the *wrong* DB.
gabber's DB carried live lifecycle rows for two foreign tasks:

| Task | Home project | Evidence in gabber's DB |
|---|---|---|
| `cas-0114` "Fix dedup migration: legacy 'due' reminder logs…" | abundant-mines | `task_lease_history` id 418 `claimed` + 419 `released` by agent `7692cd82-…` on **2026-05-02**; `verifications` row `ver-d683ca4f9862` "Closed via supervisor bypass" |
| `cas-76db` "Fix Playwright credits-v1 test quality issues from code review" | cas-src | `task_lease_history` id 174/175 by agent `e8b7ff9b-…` on **2026-04-10**; `verifications` row `ver-be71` |

A session whose CWD was gabber-studio claimed, worked and closed tasks that belong to
other repos. Corroborating: the gabber repo working tree currently contains accounting
deliverables (`docs/reports/2026-07-29-stripe-entity-migration-ben.html`), i.e. non-gabber
work *has* been done from this directory.

**Ask:** on `task start`, warn when the task's `id` is not present in the current
project's native set (e.g. no `target_repo` match and no local dependency edge), rather
than silently leasing it.

### 3.4 Status drift makes replicas actively misleading (P2)

Because the replicas were pulled once and then never re-scoped, they froze mid-flight and
now contradict the authoritative rows:

| Task | gabber (before surgery) | Home DB |
|---|---|---|
| `cas-7fa8` QBO Cleanup — Roark 2022 | `open` | **closed** in `Richards LLC/Accounting` |
| `cas-e9e9` EPIC per-worker backend/model/effort | `open` | **closed** in `cas-src` |
| `cas-7237` E4 Wallet QA / store submission | `open` | **closed** in `ozer` |
| `cas-84a9` CAS Remote Deployment & Slack Bridge | `open` | **closed** in `cas-src` |

Anyone reading gabber's queue was being told that finished work was still outstanding.

## 4. Reproduction hypothesis

On a cas-cli **older than v2.15.0**, with two or more projects sharing one personal
cloud token:

1. `cd projA && cas cloud sync` — pushes projA's tasks into the personal scope.
2. `cd projB && cas cloud pull` — the inline `ureq::get` URL omits `project_id=`, so the
   server returns every `team_id IS NULL` row in the scope, including all of projA's.
3. projB's local `tasks` table now holds projA's rows verbatim (same ids, same
   `created_at`, notes frozen at pull time).
4. Repeat across N projects → an N-way mesh; each DB holds the union.
5. Upgrade past v2.15.0. New contamination stops. **Nothing removes step 3's rows.**

Variant that still reproduces on current binaries: give two different repos the same
parent-folder name, leave `[project] canonical_id` unset in both, sync each — §3.2.

## 5. Remediation performed (gabber-studio, 2026-08-06)

* Backups (`sqlite3 .backup`, WAL-safe) written before any write:
  * `~/.cas/backup/cas.db.gabber.2026-08-06T1607`
  * `~/.cas/backup/cas.db.global.2026-08-06T1607`
  * `~/.cas/backup/cas.db.ozer.2026-08-06T1607`
* Home DB determined per group by *richest copy* (notes length + `task_lease_history`
  rows + newest `updated_at`), not by the folder name:
  * Accounting group (21) → `/home/pippenz/Richards LLC/Accounting` (notes 22,699 vs
    10,869 elsewhere; 12 closed; 4 lease rows)
  * "domdms" group (16) → **`/home/pippenz/Petrastella/abundant-mines`**, *not* domdms —
    the domdms DB holds **zero** of them; abundant-mines has notes 3,895 vs 756 and the
    only 8 `task_lease_history` rows. `PaymentReminders.vue` / `CurrentPlansList.vue`
    live in abundant-mines.
  * Telehealth group (10) → `/home/pippenz/Petrastella/ozer` (4 closed, notes 10,904;
    `ozer/.cas/worktrees/telehealth*` exist)
  * OpenClaw/Vultr → ozer; CAS-server + cas-cli groups → cas-src
* `cas-2e0b` was absent from ozer → row copied in (full-column INSERT) before deletion.
* 125 task rows + 1 dependency row + 4 `task_lease_history` rows + 2 `verifications`
  rows deleted from gabber's DB; 78 of the same ids deleted from `~/.cas/cas.db`.
* Verified: 0 rows with a gabber copy strictly newer *and* with longer notes than the
  best home copy → no content was lost by preferring the home copy.
* Post-surgery: gabber `tasks` 2,896 → 2,771; non-closed 218 → 150; `task ready`
  124 tasks, **zero foreign**; 0 orphaned `dependencies` rows.

**Not remediated:** the ~1,600 remaining shared-blob rows (all `closed`, spanning many
projects, not attributable without per-row domain review) in gabber's DB and in every
other project DB on this machine. That is what §3.1 is asking for.

### 5.1 The mirror image — gabber's tasks are polluting the other projects

Post-surgery, gabber still shares 1,204–1,690 rows with each other project DB. Almost all
are `closed`, but the **non-closed** remainder is now entirely gabber-native work sitting
in *other* projects' queues:

| Other project DB | non-closed rows shared with gabber | what they are |
|---|---|---|
| cas-src | 12 | gabber KB `T6/T7/T9` series, CreatorText tests, `cas-9d74` cancellation epic |
| `Richards LLC/Accounting` | 8 | same gabber KB `T6/T7/T9` series + IG gate-decider bugs |
| `Petra Stella/Accounting` | 8 | ditto |
| ozer | 10 | ditto + `cas-7932`, `cas-a4b8` |
| abundant-mines | 9 | ditto + `cas-42bd` |

So Ben's accounting queue currently shows "T7 — Apply KB Prisma migration to production
Neon branch". The same `purge-foreign` sweep asked for in §3.1 needs to run **in every
project**, not just the one that complained.

---

## 6. Machine-wide decontamination (2026-08-06, follow-up task cas-ee3f)

### 6.1 A new finding that invalidates the obvious fix: task ids COLLIDE

Before purging anything machine-wide we tested the assumption every purge tool relies on —
"same task id in two DBs ⇒ replica". **It is false.** CAS ids are 4 hex chars (~65k space);
across 39 DBs and 5,824 distinct ids, 2,265 ids appear in more than one DB:

| class | count | meaning |
|---|---|---|
| pure replica (all copies share `created_at`) | 2,149 | genuine cas-ed15 blob |
| **pure collision** | **73** | every copy is a *different task* that merely shares the id |
| **mixed** | **43** | a replicated blob task **plus** a distinct colliding task elsewhere |

Live examples: `cas-ee3f` is a gabber CAS-hygiene chore *and* pantheon's "Paywall modal on
/dashboard has no dismiss". `cas-9d74` is a gabber cancellation epic *and* cas-src's closed
"depth flag: end-to-end test + user docs". `cas-90b1` is Accounting's "FONCE eligibility
analysis" *and* pantheon's "Apply confirmed code-review fixes to Plaid epic".

`created_at` is also unusable as an identity key: `cas-76db` has the **same title** with two
different `created_at` values across DBs (`…T16:16:23` in cas-src/ozer/rocketship,
`…T17:40:32` in abundant-mines/Accounting/time-tracking/petra-stella-cloud).

**Any id-keyed purge deletes real work.** The key must be `(id, title)`.

### 6.2 `cas cloud purge-foreign` is unsafe on a stale-sync machine — REJECTED

Read of `execute_purge_foreign` (cli/cloud.rs:3112) shows it is not a filter, it is a
nuke-and-restore:

```
DELETE FROM entries; DELETE FROM tasks; DELETE FROM dependencies;
DELETE FROM rules;   DELETE FROM skills;
-- then re-pull everything from cloud
```

It deletes **all** local tasks and repopulates only what the cloud still holds for this
project. On this machine cloud task sync has been dead for months (`last_task_sync`:
gabber 2026-05-06, PS/Accounting 2026-04-23, abundant-mines 2026-04-14, domdms
2026-03-25), so a re-pull would restore a months-old subset and destroy everything created
since. Its own safety backup is `std::fs::copy` of a live WAL database — not crash-safe.

**Asks:** (a) make `--dry-run` actually print the delete set (today it prints only
`entities_before`, so it cannot be evaluated); (b) refuse to run when
`last_pull_at`/`last_task_sync` is older than some threshold, or when local rows exist that
were never pushed; (c) use `VACUUM INTO`/`.backup` rather than `fs::copy`.

### 6.3 What was actually done

Manual purge, key `(id, title)`, home elected by **local-only work artifacts** — the
`verifications`, `task_lease_history` and `dependencies` rows, which did *not* travel
through the pull leak (proved: gabber held 2 verification rows and 4 lease rows for the 125
foreign tasks it carried, and each id's dependency edges exist in exactly one DB). `events`
and `notes` were used only as weak tie-breakers because they *did* replicate. A row is
deleted only when another DB holds the same `(id, title)` **and** that DB is the elected
home; rows unique to a DB are never touched; score ties are left in place, never broken
arbitrarily.

**13,704 rows removed across 13 DBs**, plus 290 CAS self-test fixtures ("Context test task",
"MCP Protocol Test Task", "Test task for notification test", "Consolidated task test")
removed from every DB except their cas-src home. 0 orphaned dependency rows anywhere.

| DB | tasks before → after | non-closed before → after |
|---|---|---|
| `~/.cas` (global) | 1,628 → 116 | 278 → 0 |
| Petra Stella/Accounting | 1,752 → 276 | 325 → 19 |
| Richards LLC/Accounting | 1,766 → 404 | 312 → 69 |
| cas-src | 2,761 → 1,466 | 423 → 189 |
| gabber-studio | 2,772 → 1,496 | 150 → 142 |
| ozer | 2,707 → 1,264 | 388 → 91 |
| petra-stella-cloud | 2,026 → 268 | 94 → 21 |
| rocketship-template | 1,753 → 455 | 339 → 101 |
| abundant-mines | 1,440 → 293 | 194 → 29 |
| time-tracking | 1,305 → 130 | 187 → 28 |
| domdms | 445 → 236 | 115 → 60 |
| pantheon / petra_stella_tools / pulse-card | −3 / −1 / −1 | — |

**Left alone and reported, not guessed:** 158 `(id,title)` groups where no DB carries any
local-work artifact (nothing distinguishes the copies), and 210 groups present in **both**
Accounting DBs — two different clients' books that merged because they shared bucket
`Accounting`. Separating those is a human call, not a heuristic one.

### 6.4 §3.2 closed: canonical_id now pinned everywhere

`[project] canonical_id` is now explicitly pinned in **all 38 project `.cas/config.toml`
files**, all unique, zero duplicates. The colliding pairs were split:

- `/home/pippenz/Petra Stella/Accounting` → `petra-stella-accounting`
- `/home/pippenz/Richards LLC/Accounting` → `richards-llc-accounting`
- `…/Petra Stella/Accounting/Roark Realty/2022` → `petra-stella-roark-2022`
- `…/Richards LLC/Accounting/Roark Realty/2022` → `richards-llc-roark-2022`

The five projects with a recorded `last_push_canonical_id` (gabber-studio, cas-src, ozer,
abundant-mines, pantheon) were pinned to that exact value so the push rehome guard does not
fire. `~/.cas` (the user-level store) was deliberately **not** pinned — it is not a project.

Note for the upstream fix: `cas cloud project set` **requires a cloud login** and aborts
with "Not logged in" — so on 17 of these projects the canonical_id had to be written to
`config.toml` by hand. Pinning a project slug is a purely local operation and should not be
gated on authentication.
