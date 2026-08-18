# Scope: Make project_canonical_id Required on Push

**From:** Petra Stella Cloud team
**Date:** 2026-04-03
**Server status:** Ready (project_id stored + filtered, enforcement pending CLI update)
**Priority:** High — without this, 99.8% of synced entities have no project scope, making project-level pull/filtering useless

---

## Problem

The Cassy CLI sends `project_canonical_id` on push payloads, but only for a small subset of entity types. In production:

| Entity Type | Has project_id | Missing | % covered |
|---|---|---|---|
| task | 92 | 744 | 11% |
| entry | 1 | 237 | 0.4% |
| event | 0 | 36,767 | 0% |
| file_change | 0 | 2,569 | 0% |
| rule | 0 | 19 | 0% |
| skill | 0 | 3 | 0% |

This means entities from different projects (Cassy, gabber-studio, accounting, etc.) all mix together in the same user bucket. Pull responses return everything regardless of which project the user is working in. Team project memories are effectively broken for most entity types.

---

## Required Change

**Every push payload must include `project_canonical_id`.** No exceptions, no optionality.

### Value Format Change

Current format: git remote URL (e.g., `github.com/richards-llc/petra-stella-cloud`)

**New format: project folder name** (e.g., `petra-stella-cloud`)

Rationale:
- Stable across remote changes (fork, transfer, rename)
- Works for non-git projects (the `local:` hash prefix is opaque and not human-readable)
- Human-readable in logs, UI, and team project lists
- The folder name is always known — Cassy always runs inside a project directory

### Derivation Logic

```
project_canonical_id = basename of the Cassy project root directory
```

Where "Cassy project root" is the directory containing `.cas/` (or the git root if `.cas/` is at git root). This is already resolved in the CLI for other purposes.

Examples:
- `/home/user/projects/petra-stella-cloud/.cas/` -> `petra-stella-cloud`
- `/home/user/Petrastella/cas/.cas/` -> `cas`
- `/home/user/gabber-studio/.cas/` -> `gabber-studio`

---

## Where to Change in Cassy CLI

### 1. Project ID derivation (`cas-cli/src/cloud/config.rs`)

The `project_canonical_id()` function currently normalizes git remote URLs. Replace with:

```rust
pub fn project_canonical_id() -> Option<String> {
    let root = find_cas_root()?;
    root.file_name()
        .map(|name| name.to_string_lossy().to_string())
}
```

Drop the git-remote normalization. The folder name IS the canonical ID.

### 2. Sync push — make it required

In every push function (personal + team), change from:

```rust
let project_id = config.project_canonical_id(); // Option<String>
payload["project_canonical_id"] = project_id.map(json::Value::String).unwrap_or(json::Value::Null);
```

To:

```rust
let project_id = config.project_canonical_id()
    .ok_or_else(|| anyhow!("Cannot sync: not inside a Cassy project directory"))?;
payload["project_canonical_id"] = json::Value::String(project_id);
```

This applies to all push paths:
- `cas-cli/src/cloud/syncer/push.rs` (personal sync)
- `cas-cli/src/cloud/syncer/team_push.rs` (team sync)
- Any event/file_change push paths if they exist separately

### 3. Ensure ALL entity types include it

Verify that the push payload builder includes `project_canonical_id` at the top level for ALL batches — events, file_changes, tasks, entries, rules, skills. The server applies the same `project_id` to every entity in a push batch, so the CLI just needs to always include the field.

If events or file_changes are pushed via a different code path than tasks/entries, that path also needs the field.

---

## Migration Path

### Phase 1: CLI update (this scope)
- Change derivation to folder name
- Make it required on all pushes
- Ship CLI update

### Phase 2: Server enforcement (petra-stella-cloud, after CLI ships)
- Add 400 rejection for pushes without `project_canonical_id`
- Make `project_id` column NOT NULL in schema

### Phase 3: Backfill (optional)
- Existing 40k entities without project_id stay as-is ("unscoped")
- They'll age out naturally via event archival (14-day retention)
- Tasks/entries/rules/skills (1,003 entities) persist but can be filtered in pull responses

---

## Server-Side Contract (already implemented)

The server is ready. No server changes needed for Phase 1.

**Personal push:** `POST /api/sync/push`
```json
{
  "project_canonical_id": "petra-stella-cloud",  // REQUIRED (was optional)
  "entries": [...],
  "tasks": [...],
  "events": [...],
  "file_changes": [...]
}
```

**Team push:** `POST /api/teams/{teamId}/sync/push`
```json
{
  "project_canonical_id": "petra-stella-cloud",  // REQUIRED (was optional)
  "entries": [...],
  "tasks": [...]
}
```

**Pull filtering:** `GET /api/sync/pull?project_id=petra-stella-cloud`
- Returns only entities matching that project
- Without the param, returns all (backwards compatible)

---

## Testing

1. `cas sync` from a project directory -> push includes `project_canonical_id` = folder name
2. `cas sync` from a non-project directory -> clear error, no push
3. Pull with `?project_id=petra-stella-cloud` -> returns only that project's data
4. Team project memories endpoint -> correctly groups by new folder-name IDs
5. Verify no entity type is pushed without project_id (check all code paths)

---

## NOT in Scope

- Renaming/aliasing project IDs (e.g., two folders pointing to same project)
- Server-side enforcement (Phase 2, after CLI ships)
- Backfilling historical data
- Changing pull to require project_id (stays optional for backwards compat)

---

## Implementation Notes (Phase 1 — completed 2026-04-03)

### Changes made

| File | Change |
|---|---|
| `cas-cli/src/cloud/config.rs` | Replaced `get_project_canonical_id()` — now returns `basename(parent(.cas))` instead of normalized git remote URL or `local:<sha256>` hash. Removed `get_project_id_from_git_remote()`, `get_project_id_from_path()`, `normalize_git_remote()`. Extracted `canonical_id_from_cas_root(&Path) -> Option<String>` for testability (the main fn uses `OnceLock` so can only init once per process). Removed `std::process::Command` import (no longer shells out to `git`). |
| `cas-cli/src/cloud/mod.rs` | Removed `normalize_git_remote` from re-exports (no external callers). |
| `cas-cli/src/cloud/syncer/push.rs` | `push_sessions()` and `push_sub_batch()`: changed `if let Some(project_id)` to `ok_or_else(|| "Cannot sync: not inside a Cassy project directory")?` — push now fails early if no project ID. |
| `cas-cli/src/cloud/syncer/team_push.rs` | `push_team()`: same optional-to-required change as above. |
| `cas-cli/src/cloud/syncer/pull.rs` | Updated fallback from `"local:unknown"` to `"unknown"` in client-side entity filtering. Pull remains optional (no breaking change). |
| `cas-cli/src/cli/cloud.rs` | Updated error message from "Not in a git repository with a remote" to "Not inside a Cassy project directory". |

### Tests

- `test_canonical_id_from_cas_root` — creates real temp dirs (`petra-stella-cloud/.cas`, `gabber-studio/.cas`, `local-only-project/.cas`, `Richards LLC/.cas`) and verifies folder-name derivation
- `test_canonical_id_from_filesystem_root` — edge case: `.cas` at `/` returns `None`
- All 18 existing syncer tests + 7 pull entity-matching tests continue to pass

### Breaking change

Existing `project_canonical_id` values in the cloud DB will change format:
- **Before:** `github.com/pippenz/cas` or `local:abcd1234ef567890`
- **After:** `cas-src` or `gabber-studio`

This means the ~92 tasks and ~1 entry that already have a `project_canonical_id` will not match the new format on pull. Per Phase 3, these age out naturally or can be filtered server-side.
