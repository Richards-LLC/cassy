---
from: ozer-health (surfaced via screenshot 2026-05-13)
date: 2026-05-13
priority: P1
---

# `cas doctor --fix` auto-fix fails: `migration failed: skills_add_summary — database error: no such table: skills` on fresh-ish Cassy database

## Observed

Running `cas doctor --fix` in a project (`ozer-health`, macOS) whose `.cas/cas.db` is missing the `skills` and `agents` tables emits:

```
cas doctor
--------------------------------------------------
⚠ auto-fix: Failed to apply pending migrations: migration failed:
            skills_add_summary - database error: no such table: skills
✓ cas directory: Found at /Users/danielluchin/Repos/ozer-health/.cas
✓ database: SQLite database found
⚠ schema: v198 (81 migration(s) pending). Run 'cas update --schema-only'
⚠ tables: 9 tables (2 missing: skills, agents)
✓ entry store: 0 entries accessible
⚠ search index: Index not found. Will be created on first search.
✓ configuration: Loaded (sync: enabled)
✓ sync target: Will sync to .claude/rules/cas (not yet created)
✓ memory stats: 0 total ()
✓ rules: 0 rules ()
✓ tasks: 0 tasks () | 0 open, 0 blocked
✓ embeddings: No local vector embeddings (semantic search uses cloud).
✓ mcp config: MCP configured in .mcp.json
✓ integrations: no integrations configured
⚠ All critical checks passed with some warnings.
```

## Root cause (confirmed by code reading in `cas-src`)

The `skills` table is **created lazily** the first time `SqliteSkillStore::new()` runs:

```rust
// crates/cas-store/src/skill_store.rs:13
const SKILL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    ...
);
"#;
```

The migration framework does **not** invoke that bootstrap before running pending migrations. So if a user runs `cas doctor --fix` (or `cas update --schema-only`) on a database that:

- exists (`.cas/cas.db` is present) AND
- has not yet had `SqliteSkillStore::new()` called on it (no skill operation has triggered the lazy schema)

…then migration `m071_skills_add_summary` (an `ALTER TABLE skills ADD COLUMN summary …`) fires before the table exists and fails. The same class of failure applies to `agents` per the "2 missing: skills, agents" warning — there is presumably an analogous lazy `AgentStore::new()` bootstrap path.

There is no standalone `CREATE TABLE skills` migration prior to m071. The only `CREATE TABLE skills` statements in the migration tree live inside `m197_skills_add_share.rs` (and are guards for an unrelated case). The single source of truth for the base `skills` schema is the lazy-bootstrap constant in `crates/cas-store/src/skill_store.rs`.

## Hypotheses for the right fix

1. **(Preferred) Have the migration runner bootstrap subsystem base schemas before running ALTER migrations.** Each subsystem (`Subsystem::Skills`, `Subsystem::Agents`, etc., enumerated in `cas-cli/src/migration/mod.rs:73`) currently knows its table name. The migration runner could open the store-side schema constants (or call into `Subsystem::ensure_base_schema(&conn)`) for every enabled subsystem at the start of `apply_pending`, ahead of the ALTER chain. Single-call, idempotent thanks to `CREATE TABLE IF NOT EXISTS`.
2. **(Alternative) Promote the `CREATE TABLE skills` (and `agents`) statements into a real `m000_init_*.rs` migration** keyed at the lowest version. This makes the table creation part of the migration ledger rather than ambient state. More invasive; requires also stripping the `IF NOT EXISTS` semantics from older DBs gracefully.
3. **(Local mitigation, not a fix)** Wrap every store-touching migration with `CREATE TABLE IF NOT EXISTS skills (...)` before its `ALTER`. Touches every `m07[1-9]_skills_*.rs` and `m08[0-9]_skills_*.rs`. Worst long-term hygiene but smallest blast radius.

## What we need from cas-src

A fix (almost certainly hypothesis 1) so that:

```
$ cas doctor --fix
# on a brand-new .cas/cas.db that has never been touched by a skill-store operation
```

…succeeds without "no such table: skills" / "no such table: agents".

## Reproduction

1. Create a fresh directory.
2. Run `cas init` (or whatever creates `.cas/cas.db`) WITHOUT triggering any skill / agent operation.
3. Run `cas doctor --fix`.
4. Expected: all migrations apply cleanly.
5. Actual: `migration failed: skills_add_summary - database error: no such table: skills`.

Tighter repro path (no `cas init` shortcut needed): manually delete the `skills` and `agents` rows from `sqlite_master` of an existing DB (or `CREATE` an empty DB at `.cas/cas.db` and immediately call `cas doctor --fix`).

## Touchpoints (best guesses — please correct)

- `cas-cli/src/migration/mod.rs` — migration runner / `apply_pending` entry point
- `crates/cas-store/src/skill_store.rs` — `SKILL_SCHEMA` constant (line 13) is the canonical CREATE TABLE for `skills`
- `cas-cli/src/migration/migrations/m071_skills_add_summary.rs` — first failing migration in this chain
- Similar `agent_store.rs` / first `agents_*` migration for the parallel `agents` failure

## Acceptance

- `cas doctor --fix` succeeds on a database that has never had `SqliteSkillStore::new()` or `SqliteAgentStore::new()` invoked.
- Regression test: a unit test that opens a fresh in-memory DB, runs `apply_pending` directly (NOT via the lazy stores), and asserts all migrations apply cleanly.
- `cas update --schema-only` exhibits the same fix.

## Related

- Memory `feedback_check_upstream_before_concluding.md` — verify this is not already fixed at HEAD before scoping. As of `v2.15.0` (39f9b39), the lazy bootstrap pattern is still active in `crates/cas-store/src/skill_store.rs:13` and no migration runner bootstrap was found via grep — the bug appears live.
- Cloud-sync EPIC `cas-ffc4` is unrelated.
