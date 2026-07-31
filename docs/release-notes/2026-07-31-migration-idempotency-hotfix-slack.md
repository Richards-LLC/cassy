# 2026-07-31 — v2.38.1 migration idempotency hotfix — #cas-internal posts

## Post 1 — User

**Live on production — User** (v2.38.1)

A same-day fix for anyone updating to this morning's release. Was: on some machines the update could stop halfway through its database upgrade and refuse to retry, leaving new features dormant until manual repair. Now: the upgrade picks up exactly where it left off and finishes cleanly, every time — and even before the fix, nothing broke: the system keeps running normally on the old schema and simply reminds you to finish the update.

## Post 2 — Dev

**Live on production — Dev** (v2.38.1)

Was: migration `up` arrays executed raw `ALTER TABLE ADD COLUMN` statements guarded only by an all-or-nothing `detect`, so a DB in a mixed schema state (e.g. a table created fresh at its new shape while sibling tables were still legacy) re-ran already-applied ALTERs and died on `duplicate column name` — wedging that and every later migration. Now: the migration runner checks column existence per ADD COLUMN statement and skips those already applied, indexes use `CREATE INDEX IF NOT EXISTS`, and partial shapes converge idempotently across all multi-ALTER migrations. Regression coverage includes the live mixed-schema shape, repeat-run no-op, and a real `cas serve` subprocess proving startup with pending migrations degrades with a warning instead of failing.
