# Feature Request: Team Project Memories (Cassy CLI Side)

**From:** Petra Stella Cloud team
**Date:** 2026-04-02
**Server status:** SHIPPED (commit `92d69e8` on petra-stella-cloud main)
**Server spec:** `petra-stella-cloud/docs/FEATURE-TEAM-PROJECT-MEMORIES.md`
**Priority:** High — unblocks team knowledge sharing

---

## Context

Petra Stella Cloud now supports **team project memories**. When a developer joins a project that teammates have been working on, they can pull down the team's collective learnings (architectural decisions, bug fixes, conventions, domain knowledge) — with personal preferences automatically excluded.

The server-side is complete and deployed. This document describes what the Cassy CLI needs to implement to complete the feature end-to-end.

---

## What Already Exists in Cassy CLI

1. **`project_canonical_id`** — `cas-cli/src/cloud/config.rs:24-53` normalizes the git remote URL and includes it in push payloads. This is the project key server-side.

2. **Team push** — `cas-cli/src/cloud/syncer/team_push.rs` pushes entities with `team_id` to `/api/teams/{team_id}/sync/push`. The server now auto-registers the project in a `projects` table on first team push when `project_canonical_id` is present.

3. **Entry types** — `crates/cas-types/src/entry.rs` defines `entry_type` as `Learning`, `Preference`, `Context`, `Observation`. The server filters by this field to exclude `Preference` entries from team memory responses.

4. **Team config** — `cas-cli/src/cloud/config.rs:135-145` stores `team_id`, `team_slug`, and per-team sync timestamps in `cloud.json`.

---

## Shipped Server Endpoints

All endpoints require `Authorization: Bearer <api_key>` and team membership (returns 403 if not a member).

### GET /api/teams/{teamId}/projects

Lists all projects the team has pushed to. Projects are auto-registered when any team member pushes with a `project_canonical_id`.

**Response (200):**
```json
{
  "projects": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "canonical_id": "github.com/petrastella/cas",
      "name": "cas",
      "created_by": "user-uuid",
      "created_at": "2026-04-02T10:00:00.000Z",
      "contributor_count": 3,
      "memory_count": 147
    }
  ]
}
```

**Notes:**
- `name` is auto-derived from the last segment of `canonical_id` (e.g. `github.com/petrastella/cas` → `cas`). Can be renamed via PATCH.
- `contributor_count` = distinct users who pushed entities to this project within the team.
- `memory_count` = total entities excluding those where `data.type = 'user'` or `data.entry_type = 'Preference'`.
- Results ordered by `created_at` ascending.

### GET /api/teams/{teamId}/projects/{projectId}/memories

Returns project-scoped memories from ALL team members, with personal preferences filtered out.

**Important:** `{projectId}` is the project **UUID** from the list endpoint, NOT the canonical ID.

**Query params:**
| Param | Default | Description |
|-------|---------|-------------|
| `since` | (none) | ISO8601 timestamp — only return entities updated after this time |
| `types` | `entry,rule,skill` | Comma-separated entity types to include. Valid: `entry`, `rule`, `skill` |
| `exclude_user_type` | `true` | Set to `false` to include Preference/user-type entries |

**Response (200):**
```json
{
  "project": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "canonical_id": "github.com/petrastella/cas",
    "name": "cas"
  },
  "memories": {
    "entries": [
      { "id": "g-2026-01-15-001", "type": "Learning", "content": "...", ... },
      { "id": "g-2026-02-03-004", "type": "Context", "content": "...", ... }
    ],
    "rules": [
      { "id": "rule-uuid", "content": "...", ... }
    ],
    "skills": []
  },
  "contributors": ["user-uuid-1", "user-uuid-2"],
  "pulled_at": "2026-04-02T18:00:00.000Z"
}
```

**Error responses:**
| Status | Condition |
|--------|-----------|
| 400 | No valid entity types in `types` param |
| 401 | Missing/invalid bearer token |
| 403 | Not a member of this team |
| 404 | Project not found in this team |

**Implementation details:**
- Queries `sync_entities` WHERE `team_id = teamId AND project_id = project.canonical_id AND entity_type IN (types)`
- Privacy filter (when `exclude_user_type=true`): `NOT ((data->>'type' = 'user') OR (data->>'entry_type' = 'Preference'))`
- Results ordered by `updated_at` ascending
- Uses index `idx_sync_team_project_pull(team_id, project_id, updated_at) WHERE team_id IS NOT NULL`

### PATCH /api/teams/{teamId}/projects/{projectId}

Update project display name. Any team member can rename.

**Request body:**
```json
{ "name": "Cassy CLI" }
```

**Response (200):**
```json
{
  "id": "550e8400-...",
  "canonical_id": "github.com/petrastella/cas",
  "name": "Cassy CLI",
  "created_by": "user-uuid",
  "created_at": "2026-04-02T10:00:00.000Z"
}
```

**Errors:** 400 (missing/invalid name), 404 (project not found), 401, 403.

### Auto-Registration on Team Push

No new endpoint — this happens automatically. When `POST /api/teams/{teamId}/sync/push` receives a payload with `project_canonical_id`, it inserts into the `projects` table with `ON CONFLICT DO NOTHING`. The project name defaults to the last path segment of the canonical ID.

**What this means for the CLI:** No changes needed to push. As long as `project_canonical_id` is included in team push payloads (which it already is), projects auto-register.

---

## Requested CLI Changes

### 1. New Command: `cas cloud team-memories`

Pull memories from teammates who have worked on the current project.

```bash
cas cloud team-memories              # Pull all team memories for current project
cas cloud team-memories --dry-run    # Show what would be pulled without merging
cas cloud team-memories --full       # Ignore last sync timestamp, pull everything
```

**Flow:**
1. Read `team_id` from `cloud.json`. Error if not set.
2. Get `project_canonical_id` from current repo's git remote. Error if no remote.
3. Call `GET /api/teams/{teamId}/projects` to find the project UUID matching the canonical ID. Error if not found (project hasn't been team-pushed yet).
4. Call `GET /api/teams/{teamId}/projects/{projectId}/memories?since={last_team_memory_pull}`
5. Merge returned entries/rules/skills into local SQLite store using existing pull/merge logic from `pull.rs`
6. Store `last_team_memory_pull_at` timestamp in `cloud.json` sync metadata

**Merge behavior:**
- Use existing LWW merge from `pull.rs` — compare `updated_at` timestamps
- Team memories are read into local store as regular entries, rules, skills
- The `team_id` field on each entity is preserved so the CLI knows the origin
- If a local memory and a team memory have the same content, prefer whichever has the newer timestamp

### 2. New Command: `cas cloud projects`

List projects the team has worked on.

```bash
cas cloud projects                   # List all team projects
cas cloud projects --team <slug>     # Specify team (defaults to active team)
```

**Output format:**
```
Team: petrastella

  Cassy CLI                    github.com/petrastella/cas          3 contributors   147 memories
  Petra Stella Cloud         github.com/petrastella/cloud        2 contributors    89 memories
  Gabber Studio              github.com/petrastella/gabber       1 contributor     34 memories
```

### 3. Auto-Pull Team Memories on Session Start (Optional/Future)

If auto-sync hooks are implemented later, team memory pull could be added to the SessionStart hook alongside personal pull. Not required for the initial implementation.

### 4. New Metadata Keys in cloud.json

```json
{
  "team_memory_sync_timestamps": {
    "github.com/petrastella/cas": "2026-04-02T10:00:00Z"
  }
}
```

Keyed by `canonical_id` so each project tracks its own team memory sync state independently.

---

## Privacy Model

The server handles all privacy filtering — the CLI does not need to implement any filtering logic. The server:
- **Includes:** Learning, Context, Observation entries + all rules + all skills
- **Excludes:** Preference entries (user-specific OS/editor/workflow preferences) and entries where `data.type = 'user'`
- The CLI can override by passing `?exclude_user_type=false` if needed

If a user wants to explicitly exclude a specific memory from team visibility, that's a future feature (per-entry `private: true` flag).

---

## Edge Cases to Handle

1. **No team configured** — `cas cloud team-memories` should print: "No team configured. Run `cas cloud team set <uuid>` first." (Slug resolution was deferred when the T2 CLI shipped — see cas-4eed — because petra-stella-cloud has no slug→UUID endpoint.)

2. **Project not found on server** — First team push for this project hasn't happened yet. Print: "This project hasn't been synced to the team yet. Run `cas cloud sync` while a team is configured (see `cas cloud team set <uuid>`) to register it."

3. **No new memories** — `?since=` returns empty memories. Print: "Team memories are up to date."

4. **User's own memories in response** — The server returns ALL team members' memories including the requester's own. The merge logic handles this naturally (LWW, same entity ID = no-op if timestamps match).

5. **Large memory sets** — Server returns everything matching the filter. If a project has 10K+ memories, consider adding pagination (`?limit=500&cursor=`) in a follow-up.

---

## Testing

Server has 20 tests covering the new endpoints. CLI-side, add tests for:
- `team-memories` command with mock server responses
- Merge behavior when team memories overlap with local
- Edge cases (no team, no project, empty response)
- `projects` list command formatting

---

## Implementation Priority

1. **`cas cloud projects`** — Simple GET + format. Low risk. Lets users see what's available.
2. **`cas cloud team-memories`** — The core feature. Pull + merge.
3. **Sync metadata tracking** — Per-project timestamps in cloud.json.
4. **Auto-pull on session start** — Future, after manual flow is validated.

---

## Server Database Schema (for reference)

```sql
-- projects table (new, migration 0007)
CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  canonical_id TEXT NOT NULL,
  name TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_projects_team_canonical ON projects(team_id, canonical_id);
CREATE INDEX idx_projects_team ON projects(team_id);

-- sync_entities (existing, unchanged)
-- Key columns: user_id, entity_type, id (PK), team_id, project_id, data (JSONB)
-- Index used: idx_sync_team_project_pull(team_id, project_id, updated_at) WHERE team_id IS NOT NULL
```
