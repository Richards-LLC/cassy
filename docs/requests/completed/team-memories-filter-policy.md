---
task: cas-fda7
epic: cas-cf44
date: 2026-04-16
author: team-memories worker
status: decision
related:
  - BUG-team-memories-never-populate.md
  - completed/FEATURE-REQUEST-TEAM-PROJECT-MEMORIES.md
---

> **Disposition (2026-08-07, cas-ab75):** Decision record, not a request — no GitHub issue. Documents the team-memories filter policy and opt-out design for task cas-fda7 (epic cas-cf44, 2026-04-16); the reports it references (`BUG-team-memories-never-populate.md`, `FEATURE-cloud-sync-pull-team-memories.md`) are already in `completed/`. Archived alongside them.

# Team-memories filter policy & opt-out design

## Summary

When `cas cloud sync` runs inside a team-registered folder, the client MUST
dual-enqueue eligible writes to both the personal push queue and the team push
queue. This doc pins what "eligible" means, the opt-out surface, the treatment
of the 392 pre-existing personal entries, and the interaction with a future
`cas memory remember --share` flag.

It is the pinned input for tasks cas-82a1 (T3 dual-enqueue),
cas-1f44 (T4 team drain), cas-07d7 (T5 `cas memory share`), cas-a9a9 (T6 E2E),
and cas-19e6 (T7 docs).

## Scope vocabulary (the part the BUG spec got loose)

The BUG spec uses "personal" informally. In Cassy code the scope enum is binary:

- `Scope::Global` — stored under `~/.config/cas/`. User-level preferences,
  cross-project rules, global skills. ID prefix `g` (full IDs like
  `g-2025-01-01-001`).
- `Scope::Project` — stored under `./.cas/`. Codebase-specific learnings,
  project context, project rules/skills. ID prefix `p`.

There is no `Scope::Personal`. In this document and in all derived tasks we
use the two scope names above. When the BUG spec says "personal stays local"
it means **Global-scoped data does not auto-promote**.

Orthogonally, every entry carries `EntryType ∈ { Learning, Preference,
Context, Observation }`. `Preference` is user-level regardless of scope.

## Decision 1 — Which entities auto-promote

**Rule:** dual-enqueue to the team push queue iff ALL of the following hold:

1. The folder has a configured team (`cloud.json.team_id` set via
   `cas cloud team set`), AND
2. The entity's storage scope is `Project`, AND
3. The entity is not an intrinsically user-level type:
   - Entries: `entry_type != Preference`.
   - Rules, skills, tasks, sessions, verifications, events, prompts,
     file_changes, commit_links, agents, worktrees: no per-type exclusion —
     if it is Project-scoped, it auto-promotes.

Rationale:

- The server already strips `Preference` entries on the pull side, but
  filtering on push keeps team rows clean and avoids wasted bandwidth, and
  matters if we ever expose raw team rows (debugging, audit).
- Global-scoped data is deliberately user-level (configured editor prefs,
  general coding style learnings) and should never silently land in a team's
  shared memory even if the user happens to be inside a team-registered folder
  when they run `cas memory remember --scope global`.
- Applying the same Project-scope rule to rules/skills avoids a third policy
  axis. If Daniel writes a global rule, it stays personal; if he writes a
  project rule in `cas-src`, the team sees it. Matches the mental model users
  already have from `--scope` on `cas memory remember`.

### Implementation target for T3

In **each** of `cas-cli/src/store/syncing_entry.rs`, `syncing_task.rs`,
`syncing_rule.rs`, `syncing_skill.rs`, after the existing personal
`enqueue(...)` inside `queue_upsert` / `queue_delete`, add a sibling
`enqueue_for_team(...)` call guarded by the eligibility predicate. The
`EntityType` passed MUST match the per-store type already used on the
personal path (Entry / Task / Rule / Skill) — do not copy-paste the
`EntityType::Entry` literal from the entry-store example below into the
task or skill stores. Sketch for the entry store:

```rust
// in SyncingEntryStore
if let Some(team_id) = self.cloud_config.active_team_id()
    && eligible_for_team(entry)
{
    if let Err(e) = self.queue.enqueue_for_team(
        EntityType::Entry,
        &entry.id,
        SyncOperation::Upsert,
        Some(&payload),
        &team_id,
    ) {
        tracing::warn!(target: "cas::sync", "team enqueue failed: {e}");
    }
}
```

Structural prerequisites T3 must land first:

1. Extend `SyncingEntryStore` (and its three siblings) to hold a
   pre-resolved `team_id: Option<Arc<str>>`. Attach it via a builder
   method `with_cloud_config(cloud_config: Arc<CloudConfig>) -> Self`
   so existing `new(...)` call sites keep working (personal-only) and
   only the `detect.rs` production paths opt in. The team_id is
   pre-resolved once at `with_cloud_config` time rather than looked up
   per-write — the tradeoff is that `cas cloud team set|clear` in
   another process only takes effect when this store is reconstructed.
2. Add an `impl CloudConfig { pub fn active_team_id(&self) -> Option<&str> }`
   accessor that returns `self.team_id.as_deref()` only when
   `self.team_auto_promote != Some(false)` — this is the Decision 3 coarse
   kill-switch, enforced at the accessor so every caller sees it.
3. Add `pub team_auto_promote: Option<bool>` to `CloudConfig`
   (`cas-cli/src/cloud/config.rs`) with `#[serde(default)]`. `None` and
   `Some(true)` both mean "enabled"; `Some(false)` disables dual-enqueue.
   This is part of T3 scope, not a separate task.
4. `eligible_for_team(entity)` is a local pure predicate — no network, no
   config read beyond the `share` / `scope` / `entry_type` fields on the
   entity itself. T3 MUST unit-test the full truth table (see "Testing
   expectations" below).

The unique key `(entity_type, entity_id, team_id)` already allows personal
and team rows to coexist — see the `ON CONFLICT` clause at
`cas-cli/src/cloud/sync_queue/queue_ops.rs:49`.

The `let _ =` pattern used on the personal enqueue path (a documented
"best-effort" choice) is deliberately not replicated here: team-scope data
loss is harder to re-derive than personal because it blocks teammates, so
failures MUST be logged via `tracing::warn!` at minimum.

Condition 1 of the auto-promote rule ("team_id set via `cas cloud team set`")
depends on T2 (cas-4eed) landing the CLI writer for `team_id`. Until T2
ships, T3 can exercise the predicate with a manually-populated
`cloud.json.team_id` for integration smoke tests.

### Testing expectations for T3

`eligible_for_team` is a pure predicate — unit-testable directly. T3 MUST
include table-driven tests covering every row of the Decision 2 precedence
table:

| `share`   | `scope`  | `entry_type`    | expected |
|-----------|----------|-----------------|----------|
| `Private` | Project  | Learning        | false    |
| `Private` | Global   | Learning        | false    |
| `Team`    | Global   | Preference      | true     |
| `Team`    | Project  | Learning        | true     |
| None      | Project  | Learning        | true     |
| None      | Project  | Preference      | false    |
| None      | Global   | Learning        | false    |

Plus, at the `queue_upsert` integration level:

- `team_auto_promote = Some(false)` → no team row enqueued for any input.
- `team_id = None` → no team row enqueued.
- Happy path (Project + Learning, team set, auto-promote enabled) → both
  personal and team rows present in `sync_queue`.

The `team_auto_promote = Some(false)` kill-switch must have its own
regression test — a missed `false` branch silently leaks data to the team
queue.

## Decision 2 — Per-entry opt-out: `share` field on Entry

Add a new serde-optional field to `Entry` (and the parallel shape on Rule,
Skill):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub share: Option<ShareScope>,

pub enum ShareScope {
    /// Explicit personal — never auto-promoted regardless of scope/type.
    Private,
    /// Explicit team — force-promoted even if the default rule would skip it.
    Team,
}
```

Precedence at dual-enqueue time:

| `share`       | `scope`  | `entry_type` | auto-promote? |
|---------------|----------|--------------|---------------|
| `Private`     | any      | any          | **no**        |
| `Team`        | any      | any          | **yes**       |
| unset (None)  | Project  | not Preference | **yes**     |
| unset (None)  | Project  | Preference   | no            |
| unset (None)  | Global   | any          | no            |

`Team` as an explicit override exists for the rare case where a user wants to
share a Global-scope learning with the team (e.g., "the team uses spaces, not
tabs") without reclassifying it as project-scoped. This is deliberate surface
for T5 (`cas memory share <id>`) — setting `share = Team` on an existing entry
is the retroactive-promotion primitive, and the sync wrappers then dual-enqueue
on the next update.

`Private` is the escape hatch for users who want a project-scoped learning to
stay local ("I tried X and it didn't work — keep this to myself").

**Asymmetry with the server's pull-side filter:** the server's team-memories
endpoint strips `entry_type = Preference` on pull (the privacy model, per
FEATURE-REQUEST). So the `share = Team, entry_type = Preference` cell of
the precedence table is "enqueued yes, visible on pull no". To avoid a
silent CLI/server mismatch — where a user runs `cas memory share <pref-id>`,
sees success, and teammates never see it — T5 (`cas memory share`) MUST:

- Reject (or loudly warn) when the target entry has `entry_type =
  Preference`, OR
- Print "Note: Preference entries are stripped by the server on pull;
  teammates will not see this entry" after a successful share.

T3's `eligible_for_team` does NOT reject this combo (the row is still
correctly enqueued for audit / future server-side policy changes); the
user-facing guard lives in T5 where the user-intent is actually expressed.

### Why not a boolean `private: bool`?

Two reasons:

1. A tri-state `Option<ShareScope>` lets T5 implement `cas memory share` and
   `cas memory unshare` as symmetric operations: `share` sets `Team`,
   `unshare` sets `Private`, both are explicit. A boolean would make `unshare`
   ambiguous — does it mean "clear the override" or "force private"?
2. It leaves room for future values (`Org`, `Public`, `Team:specific-team`)
   without a breaking type change.

## Decision 3 — Config-level opt-out: `team_auto_promote`

Add to `cloud.json`:

```json
{
  "team_auto_promote": true
}
```

Default: `true` when `team_id` is set, otherwise irrelevant. When explicitly
set to `false`, `eligible_for_team` returns `false` for all entities —
effectively disables dual-enqueue at the project level, leaving only explicit
`cas memory remember --share team` / `cas memory share <id>` as push paths.

Rationale:

- Users on a team who are doing exploratory / spike work in a project may want
  to opt out temporarily without unregistering the team.
- It's the least disruptive way to kill the feature entirely for users who
  turn out to hate it.
- Per-entry `share` covers the fine-grained case; this covers the coarse case.

### Non-goals for config-level opt-out

- No per-entry-type opt-out in config (e.g., "push rules but not entries").
  Keeps config flat. If someone needs it, they use `share = Private` per-entry.
- No regex / path-based filters. Same reason.

## Decision 4 — Pre-existing 392 personal entries

**They do not auto-migrate.** On the first sync after T3/T4 ship:

- New writes (adds/updates/deletes after the upgrade) are dual-enqueued per
  Decision 1. This backfills naturally as the user works.
- Historical entries with `team_id = NULL, project_id = NULL` stay put. They
  are surfaced by T5 (`cas memory share --since 7d`) for retroactive
  promotion — it's user-driven, not implicit.

Rationale:

- Auto-promoting 392 entries on first sync after install is a privacy-surprise
  event: the user did not consent to each one when they wrote it. "Silent mass
  share with teammates" is exactly the kind of default that erodes trust.
- The user may not be on the same team they were "conceptually" on when they
  wrote those entries (team assignment is new).
- T5 can make retroactive promotion cheap and legible (`--dry-run`,
  `--since`, tag/type filter) — there's no reason to make the default silent
  and unreviewable.

### What about legitimately project-scoped old entries?

T5 spec should include:

```
cas memory share --since 30d --dry-run
# prints: "47 Project-scoped, non-Preference entries would be shared"
cas memory share --since 30d
```

That's the replacement for auto-backfill. One command, auditable, reversible
via `cas memory unshare`.

## Decision 5 — `cas memory remember --share <scope>` surface

Defer to T2 (cas-4eed, registers `cas cloud team set|show|clear` — the CLI
writer for `cloud.json.team_id` that Decision 1 condition 1 depends on) and
T5 (cas-07d7, adds `cas memory share|unshare|remember --share` — the
read/write surface for the `share` field this decision introduces) for
final CLI shape. Pin the semantics now:

- `cas memory remember --share team "..."` → creates entry with
  `share = Some(Team)` regardless of `--scope`.
- `cas memory remember --share personal "..."` → creates entry with
  `share = Some(Private)`.
- `cas memory remember "..."` (no flag) → `share = None`; auto-promotion rule
  applies based on scope + type.
- `cas memory remember --scope global "..."` → `scope = Global, share = None`
  → does NOT auto-promote (Decision 1). Combined `--scope global --share team`
  is legal and force-promotes.

Mental model the user can hold in their head:

> `--scope` is *where the data lives* (local filesystem).
> `--share` is *who can see it* (sync target).

Keeping the two axes orthogonal is the whole point of using `Option<ShareScope>`
as an override rather than a third scope value.

## Decision 6 — Rules, skills, other entity types

- Rules: Project-scope rules dual-enqueue per Decision 1. No `entry_type`
  filter applies (rules don't have preference-equivalents). `share` field
  added to rule schema analogously for per-rule override.
- Skills: same as rules.
- Tasks: Project-scoped tasks dual-enqueue. Task visibility within a team is
  already the user's intent when they registered the folder as a team project
  — tasks are collaborative by nature.
- Sessions, verifications, events, prompts, file_changes, commit_links,
  agents, worktrees: dual-enqueue when Project-scoped. These are lightweight
  telemetry the server uses to render activity views; per-entity opt-out
  isn't worth the UI surface. (Config-level `team_auto_promote=false` still
  kills them.)

## What this doc does NOT decide

- The exact shape of the `cas memory share` CLI (T5 owns that, including
  `--since`, `--tag`, `--dry-run` flags).
- Migration of the `share` column at the server side — the server already
  stores `team_id` per row; the `share` field lives in the entry JSON and the
  server doesn't need to inspect it. No server change required.
- How `cas cloud sync` reports push results to the user when dual-enqueue
  pushes two copies. The push/drain tasks (T4) should count team and personal
  separately in their summary.
- The UX of `cas cloud team set|show|clear` (T2 owns).

## Acceptance-criteria checklist (from task cas-fda7)

- [x] (a) Which `data.scope` values auto-promote to team — Decision 1:
      `Project` only, `Global` stays local.
- [x] (b) Opt-out flag/config shape — Decision 2 (`share: Option<ShareScope>`
      on entry) + Decision 3 (`team_auto_promote: bool` in cloud.json).
- [x] (c) Behavior on pre-existing 392 personal entries — Decision 4: no
      auto-migration; surfaced via T5 `cas memory share --since`.
- [x] (d) Interaction with `cas memory remember --share personal/team` —
      Decision 5: `--share` sets `share` field, overrides auto-rule, orthogonal
      to `--scope`.
