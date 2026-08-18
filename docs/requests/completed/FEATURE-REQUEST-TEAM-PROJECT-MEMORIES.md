# Feature Request: Team Project Memories — End-to-End Integration

**From:** Petra Stella Cloud team
**Date:** 2026-04-12 (replaces 2026-04-02 version)
**Server status:** SHIPPED and deployed on Vercel
**CLI status:** Commands exist (`cas cloud projects`, `cas cloud team-memories`) but have never worked in production — blocked by sync queue drain bug (see BUG-CLOUD-SYNC-QUEUE-NEVER-DRAINS.md)
**Priority:** High — this is the flagship team collaboration feature

---

## Summary

Team project memories let developers pull down their teammates' collective learnings — architectural decisions, bug fix insights, domain knowledge, conventions — scoped to the project they're currently working in. Personal preferences are automatically filtered out.

The server-side is complete. The CLI commands exist. But the feature has **never functioned end-to-end** because the sync queue drain bug prevents data from reaching the cloud. Once that fix ships, this feature should activate with minimal additional work — but there are format changes and integration gaps to address.

---

## What Changed Since the Original Spec

| Area | Old (2026-04-02) | Current (2026-04-12) | Impact |
|------|-------------------|----------------------|--------|
| `project_canonical_id` format | Git remote URL (`github.com/richards-llc/cas`) | Folder name (`cas-src`) | Server auto-derives display name from last `/` segment — works for both formats. CLI derivation updated in `config.rs`. |
| Server push validation | Accepted NULL `project_canonical_id` | **Rejects** NULL/empty with 400 | CLI already sends it on every push. No breakage. |
| Push response | `{ synced: { entries: N } }` | `{ synced: { entries: { inserted, updated, skipped } } }` | CLI should parse new shape for better reporting |
| Stale data in cloud | ~40k entities with NULL `project_id` | Orphaned NULL-project rows deleted (10,450 purged) | Clean slate for new pushes |
| CLI commands | Proposed in spec | **Implemented**: `cas cloud projects`, `cas cloud team-memories`, `cas cloud purge-foreign` | Need integration testing once sync works |

---

## Architecture

```
Developer A (cas-src project)
  └─ cas cloud sync --team
       └─ POST /api/teams/{teamId}/sync/push
            ├─ project_canonical_id: "cas-src"
            ├─ entries: [Learning, Context, Observation, ...]
            ├─ rules: [...]
            └─ skills: [...]
                 └─ Server: upsert sync_entities + auto-register project

Developer B (same project, different machine)
  └─ cas cloud team-memories
       ├─ GET /api/teams/{teamId}/projects          → find project UUID
       └─ GET /api/teams/{teamId}/projects/{id}/memories?since=...
            └─ Server returns cross-user memories (Preferences excluded)
                 └─ CLI merges into local SQLite via LWW
```

---

## Server API Contract (Current, Deployed)

All endpoints require `Authorization: Bearer <api_key>` and team membership (403 if not a member).

### POST /api/teams/{teamId}/sync/push

Pushes entities to cloud. Auto-registers the project on first push.

**Request:**
```json
{
  "project_canonical_id": "cas-src",
  "entries": [{ "id": "...", "updated_at": "...", ...entity_data }],
  "tasks": [...],
  "rules": [...],
  "skills": [...],
  "sessions": [...],
  "verifications": [...],
  "events": [...],
  "prompts": [...],
  "file_changes": [...],
  "commit_links": [...],
  "agents": [...],
  "worktrees": [...]
}
```

**Auto-registration logic:**
```typescript
await db.insert(projects).values({
  teamId,
  canonicalId: projectId,
  name: projectId.split("/").pop() ?? projectId,
  createdBy: user.id,
}).onConflictDoNothing();
```

Note: `name` derivation uses `split("/").pop()` which works for both old format (`github.com/foo/bar` -> `bar`) and new folder-name format (`cas-src` -> `cas-src`).

**Upsert behavior:** LWW — updates only when `excluded.updated_at > sync_entities.updated_at`.

**Response:**
```json
{
  "synced": {
    "entries": { "inserted": 5, "updated": 2, "skipped": 0 },
    "tasks": { "inserted": 12, "updated": 0, "skipped": 3 },
    "rules": { "inserted": 0, "updated": 1, "skipped": 0 }
  }
}
```

**Errors:** 400 (missing `project_canonical_id`, bad JSON), 402 (plan limit), 413 (payload too large), 429 (rate limit: 60 req/60s/user), 401/403 (auth).

### GET /api/teams/{teamId}/projects

Lists all projects the team has pushed to.

**Response:**
```json
{
  "projects": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "canonical_id": "cas-src",
      "name": "cas-src",
      "created_by": "user-uuid",
      "created_at": "2026-04-12T10:00:00.000Z",
      "contributor_count": 3,
      "memory_count": 147
    }
  ]
}
```

**Notes:**
- `contributor_count` = distinct users who pushed entities to this project within the team
- `memory_count` = total entities excluding those where `data->>'type' = 'user'` OR `data->>'entry_type' = 'Preference'`
- Ordered by `created_at` ascending

### GET /api/teams/{teamId}/projects/{projectId}/memories

Returns project-scoped memories from ALL team members with privacy filtering.

**Important:** `{projectId}` is the project **UUID** from the list endpoint, NOT the canonical ID.

**Query params:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `since` | ISO 8601 timestamp | (none) | Only return entities updated after this time |
| `types` | comma-separated | `entry,rule,skill` | Entity types to include. Valid: `entry`, `rule`, `skill` |
| `exclude_user_type` | `true`/`false` | `true` | Exclude user-type entries and Preference entries |

**Response:**
```json
{
  "project": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "canonical_id": "cas-src",
    "name": "cas-src"
  },
  "memories": {
    "entries": [
      { "id": "g-2026-04-10-001", "type": "Learning", "content": "...", ... },
      { "id": "g-2026-04-11-004", "type": "Context", "content": "...", ... }
    ],
    "rules": [
      { "id": "rule-uuid", "content": "...", ... }
    ],
    "skills": []
  },
  "contributors": ["user-uuid-1", "user-uuid-2"],
  "pulled_at": "2026-04-12T18:00:00.000Z"
}
```

**Privacy filter SQL:**
```sql
NOT ((data->>'type' = 'user') OR (data->>'entry_type' = 'Preference'))
```

**Errors:** 400 (no valid types), 401, 403, 404 (project not found in team).

**Index used:** `idx_sync_team_project_pull(team_id, project_id, updated_at) WHERE team_id IS NOT NULL`

### PATCH /api/teams/{teamId}/projects/{projectId}

Rename a project. Any team member can rename.

**Request:** `{ "name": "Cassy CLI" }`

**Response:**
```json
{
  "id": "550e8400-...",
  "canonical_id": "cas-src",
  "name": "Cassy CLI",
  "created_by": "user-uuid",
  "created_at": "2026-04-12T10:00:00.000Z"
}
```

---

## CLI Commands (Existing Implementation)

### `cas cloud projects`

Lists team projects. Already implemented.

```bash
cas cloud projects                   # List all team projects
cas cloud projects --team <slug>     # Specify team (defaults to active team)
```

**Expected output:**
```
Team: petrastella

  cas-src                    cas-src              3 contributors   147 memories
  Petra Stella Cloud         petra-stella-cloud   2 contributors    89 memories
  Gabber Studio              gabber-studio        1 contributor     34 memories
```

### `cas cloud team-memories`

Pulls memories from teammates. Already implemented.

```bash
cas cloud team-memories              # Pull team memories for current project
cas cloud team-memories --dry-run    # Preview without merging
cas cloud team-memories --full       # Ignore last sync timestamp, pull everything
```

**Flow:**
1. Read `team_id` from `.cas/cloud.json`. Error if not set.
2. Get `project_canonical_id` from folder name derivation.
3. `GET /api/teams/{teamId}/projects` — find project UUID matching canonical ID.
4. `GET /api/teams/{teamId}/projects/{projectId}/memories?since={last_team_memory_pull}`
5. Merge into local SQLite using configurable conflict resolution strategy.
6. Update `team_memory_sync_timestamps[canonical_id]` in `cloud.json`.

**Merge behavior:**
- Default strategy: `RemoteWins` for team sync
- Configurable via `CloudSyncerConfig::team_conflict_resolution`
- Strategies: `RemoteWins` (always take remote), `LocalWins` (keep local), `KeepRecent` (LWW by timestamp)
- Same-ID entities resolved via strategy; new entities inserted directly

### `cas cloud purge-foreign`

Removes entities from other projects and re-pulls. Already implemented.

```bash
cas cloud purge-foreign              # Purge and re-pull
cas cloud purge-foreign --dry-run    # Preview without action
```

**Process:**
1. Back up database to `.cas/cas.db.pre-purge-{timestamp}`
2. Delete all entries/tasks/rules/skills via SQL
3. Reset `last_pull_at` metadata
4. Re-pull from cloud with project filtering
5. Preserves sync_queue, sessions, verifications, events

---

## What Needs to Happen Once Sync Fix Lands

### Immediate (automatic)

Once the sync queue drain fix ships (BUG-CLOUD-SYNC-QUEUE-NEVER-DRAINS.md fixes 1-3), the following happens automatically:

1. **Session start** triggers a push — drains 353+ queued items from petra-stella-cloud, 101 from cas-src, etc.
2. **Push payloads** include `project_canonical_id` in folder-name format (`cas-src`, `petra-stella-cloud`)
3. **Server auto-registers** each project in the `projects` table on first team push
4. **Team memories become available** via `GET /api/teams/{teamId}/projects/{projectId}/memories`

No CLI code changes required for this path.

### Verify (manual, one-time)

After the first successful sync post-fix:

1. **Check cloud data populated:**
   ```bash
   cas cloud status          # Confirm last_push_at is recent
   cas cloud projects        # Verify projects appear with memory counts
   ```

2. **Test team memories pull:**
   ```bash
   cas cloud team-memories --dry-run    # Preview what would merge
   cas cloud team-memories              # Pull and merge
   ```

3. **Verify privacy filtering:**
   - Confirm no `Preference` entries appear in team memories
   - Confirm no `type: "user"` entries appear
   - Verify `contributor_count` reflects actual distinct pushers

4. **Verify project_canonical_id format:**
   - Cloud DB should show `project_id` = folder names (e.g., `cas-src`), not git URLs
   - `cas cloud projects` should show clean names, not `github.com/...`

### CLI Updates Needed

These are non-blocking improvements to address once the core flow is verified:

#### 1. Parse new push response shape

The server now returns per-type `{ inserted, updated, skipped }` instead of flat counts. If the CLI currently expects the old `{ entries: N }` shape, update the response parser.

**Files:** `cas-cli/src/cloud/syncer/push.rs`, `team_push.rs`

**Old:** `synced.entries` is a number
**New:** `synced.entries` is `{ inserted: N, updated: N, skipped: N }`

**Priority:** Medium — parsing might already handle unknown shapes gracefully, but the CLI should report skipped rows to the user.

#### 2. Handle skipped rows

When the server skips rows (e.g., older-than-existing timestamps), the CLI should:
- Log skipped counts at info level
- Not treat skipped rows as errors
- Not re-queue skipped items (they were intentionally skipped by LWW)

**Priority:** Medium — prevents confusing "N items synced" counts when some were no-ops.

#### 3. Auto-pull team memories on session start

Currently `pull_on_start` triggers a personal pull but not a team memory pull. Add an option:

```json
// cloud.json
{
  "team_memory_pull_on_start": true
}
```

When enabled, session start would:
1. Personal push (drain queue)
2. Personal pull (with project_id filter)
3. Team memory pull (if team_id configured)

**Priority:** Low — manual `cas cloud team-memories` works first. Add auto-pull after the manual flow is validated.

#### 4. Pagination for large memory sets

The memories endpoint returns all matching entities. For projects with 10K+ memories, this could be slow. Consider:
- Client-side: cap at 500 items per pull, use `since` for incremental
- Server-side: add `?limit=500&cursor=` params (not yet implemented)

**Priority:** Low — unlikely to hit this limit in early usage.

---

## Privacy Model

The server handles all privacy filtering. The CLI does not need to implement any filtering logic.

**Included in team memories:**
- `Learning` entries — bug fixes, architectural decisions, patterns
- `Context` entries — project context, domain knowledge
- `Observation` entries — auto-captured from hooks
- All rules
- All skills

**Excluded from team memories:**
- `Preference` entries — user-specific editor/OS/workflow preferences
- Entries where `data.type = 'user'` — user profile data

**Future:** Per-entry `private: true` flag for explicit exclusion (not yet implemented on either side).

---

## Database Schema (Server, Current)

### projects table

```sql
CREATE TABLE projects (
  id TEXT PRIMARY KEY,                                    -- UUID
  team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  canonical_id TEXT NOT NULL,                             -- folder name (e.g., "cas-src")
  name TEXT NOT NULL,                                     -- display name, renameable
  created_by TEXT NOT NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_projects_team_canonical ON projects(team_id, canonical_id);
CREATE INDEX idx_projects_team ON projects(team_id);
```

### sync_entities table (relevant columns)

```sql
CREATE TABLE sync_entities (
  id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  user_id TEXT NOT NULL,
  team_id TEXT,                                           -- NULL for personal, set for team
  project_id TEXT,                                        -- canonical folder name
  data JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, entity_type, id)
);

-- Index used by team project memories
CREATE INDEX idx_sync_team_project_pull
  ON sync_entities(team_id, project_id, updated_at)
  WHERE team_id IS NOT NULL;
```

---

## CLI Data Structures (Reference)

### cloud.json (team memory fields)

```json
{
  "team_id": "team-456",
  "team_slug": "engineering",
  "team_sync_timestamps": {
    "team-456": "2026-04-12T10:30:00Z"
  },
  "team_memory_sync_timestamps": {
    "cas-src": "2026-04-10T15:45:00Z",
    "petra-stella-cloud": "2026-04-09T09:00:00Z"
  }
}
```

### Entry types relevant to filtering

```rust
pub enum EntryType {
    Learning,      // Included in team memories
    Preference,    // EXCLUDED from team memories
    Context,       // Included
    Observation,   // Included
}
```

### Conflict resolution

```rust
pub enum ConflictResolution {
    RemoteWins,   // Default for team sync — always take teammate's version
    LocalWins,    // Keep your version
    KeepRecent,   // LWW by updated_at timestamp
}
```

---

## Edge Cases

| Scenario | Expected Behavior |
|----------|-------------------|
| No team configured | `cas cloud team-memories` prints: "No team configured. Run `cas cloud team set <slug>` first." |
| Project not pushed to team yet | "This project hasn't been synced to the team yet. Run `cas cloud sync --team` to register it." |
| No new memories since last pull | "Team memories are up to date." |
| User's own memories in response | Handled by LWW merge — same entity ID = no-op if timestamps match |
| Mixed old/new `project_canonical_id` format | Server stores whatever the CLI sends. Old git-URL format rows will coexist with new folder-name rows but won't match on pull (different `project_id`). They age out naturally via event archival (14-day retention for events) or persist harmlessly for entries/rules. |
| Multiple folders with same name | Different users with `~/projects/cas-src` and `~/work/cas-src` would share the same `project_canonical_id`. This is acceptable — team scoping means only team members see each other's data. |
| Folder renamed | New `project_canonical_id`, new project record. Old data stays under old name. No migration path currently. |

---

## Testing Plan

### Integration tests (once sync fix ships)

1. **Push from two team members on same project:**
   - User A: `cas cloud sync --team` from `~/cas-src/`
   - User B: `cas cloud sync --team` from `~/cas-src/`
   - Verify: `cas cloud projects` shows 2 contributors

2. **Pull team memories:**
   - User A pushes Learning entries
   - User B: `cas cloud team-memories`
   - Verify: User A's learnings appear in User B's local store

3. **Privacy filter:**
   - User A pushes a Preference entry ("I use vim")
   - User B: `cas cloud team-memories`
   - Verify: Preference entry is NOT in the response

4. **Incremental sync:**
   - User B pulls team memories (records timestamp)
   - User A pushes new entries
   - User B pulls again with `?since=` timestamp
   - Verify: only new entries returned

5. **Dry run:**
   - `cas cloud team-memories --dry-run`
   - Verify: shows what would merge, does not modify local store

6. **Project rename:**
   - Call `PATCH /api/teams/{teamId}/projects/{projectId}` with new name
   - Verify: `cas cloud projects` shows updated name

### Automated tests (CLI-side)

- Mock server responses for all three endpoints
- Test LWW merge with overlapping local/remote entries
- Test all three conflict resolution strategies
- Test edge cases: no team, no project, empty response
- Test `cloud.json` timestamp persistence after pull

---

## Dependencies

| Dependency | Status | Blocks |
|------------|--------|--------|
| Sync queue drain fix (BUG-CLOUD-SYNC-QUEUE-NEVER-DRAINS.md) | **In progress** — Cassy team implementing | Everything. No data flows to cloud without this. |
| `project_canonical_id` = folder name (SCOPE-PROJECT-ID-REQUIRED.md Phase 1) | **Shipped** in CLI | None — already merged |
| Server rejects NULL `project_canonical_id` | **Shipped** in cloud (cloud-d656) | None — already deployed |
| Server returns `{ inserted, updated, skipped }` | **Shipped** in cloud (cloud-f645) | CLI response parsing (non-blocking) |
| Server orphan data cleanup | **Done** — 10,450 rows purged | None |

---

## NOT in Scope

- Server-side `project_id` NOT NULL migration (Phase 2 — after CLI fix is confirmed working)
- Per-entry `private: true` flag for explicit team exclusion
- Project aliasing/renaming canonical IDs
- Cross-team memory sharing
- Pagination on memories endpoint (premature — assess after real usage data)
- Real-time memory push notifications (WebSocket — future)
