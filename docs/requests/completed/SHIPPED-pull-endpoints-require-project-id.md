---
from: Petra Stella Cloud team
date: 2026-06-26
priority: P1
re: BUG-pull-endpoints-still-accept-unscoped-requests.md (your filing, 2026-06-26)
related_cas: cas-2eb3 (cross-project contamination epic), cas-990b (FEATURE-mandatory-project-id-on-pull)
cloud_task: cas-86bb
commit: 077d85a
status: SHIPPED + verified in prod
---

# SHIPPED — pull endpoints now require `project_id` (server half of cas-2eb3)

**To:** cas-src team (daniel@petrastella.io)
**From:** Petra Stella Cloud team
**Date:** 2026-06-26

Your P1 `BUG-pull-endpoints-still-accept-unscoped-requests.md` is fixed and **live in production**. The unshipped server gate from `FEATURE-mandatory-project-id-on-pull.md` (cas-990b) shipped with it. The pull endpoints no longer answer unscoped requests with the full personal/team-scope dump.

## What shipped

PR **#37**, squash-merged to `main` as commit **`077d85a`** (`fix(sync): cas-86bb require project_id on pull endpoints; bump MIN_CLIENT_VERSION`). Deployed to `petra-stella-cloud.vercel.app`.

- **`/api/sync/pull` and `/api/teams/[teamId]/sync/pull`** now require a non-empty `project_id` query param. It is trimmed; whitespace-only is treated as missing. Missing/empty → **`400 {"error":"project_id query parameter is required"}`**, returned **before any SELECT** (zero rows can leak), logged via `reqLog.error(400, …)`. A correctly-scoped pull is unchanged — same rows as before.
- **Optional `client_version` query gate** added to both pull routes, mirroring push (`compareSemver` + `isNaN` invalid-format guard → 400). Note current clients don't send `client_version` on the pull wire, so the real pull enforcement is `project_id`; the version gate is defense-in-depth for any future call site that does send it.
- **`MIN_CLIENT_VERSION` bumped `2.0.0 → 2.20.0`** (`lib/version.ts`). The version gate logic was extracted into a single `clientVersionError()` helper that **all four** sync push/pull handlers now call, so the push and pull gates can no longer drift. `/api/version` now reports `min_client_version: 2.20.0`.

## ⚠️ One behavior change worth flagging to cas-src

The `2.20.0` bump also tightens the **push** gate (same shared constant). Any client that sends `client_version` **< 2.20.0** on push now gets a `400 "… below minimum 2.20.0. Please upgrade."` instead of a silent no-op. Current release is `v2.23.1`, so no supported client is affected — but if any old binaries are still pushing in the wild, they will now fail loudly (which is the intended cas-2eb3 outcome). Clients that send **no** `client_version` are unaffected (warn-only, unchanged).

## Acceptance criteria — all met

| AC | Status |
|---|---|
| 1. Unscoped pull → 400 `project_id query parameter is required` (both endpoints), logged | ✅ |
| 2. `project_id` trimmed; whitespace-only treated as missing | ✅ |
| 3. `client_version` below floor → 400 (mirrors push); `MIN_CLIENT_VERSION` bumped off `2.0.0` | ✅ (→ 2.20.0) |
| 4. Correctly-scoped pull unchanged | ✅ |
| 5. Integration test: unscoped pull rejected (400, zero rows) + scoped returns only requested project | ✅ |

## Proof

- `pnpm build` → clean; `pnpm test` → **56 files / 472 tests passed** (new `tests/api/sync/pull-require-project.test.ts`: personal + team, unscoped/whitespace → 400 with `db.select` never called, scoped → 200 passthrough, stale/malformed `client_version` → 400).
- Prod smoke: `GET https://petra-stella-cloud.vercel.app/api/version` → `{"server_version":"1.0.0","min_client_version":"2.20.0"}` (confirms the deploy carrying these route changes is live). Both pull routes serve and require auth (401 unauthenticated); the `project_id` 400 gate sits behind auth and is pinned by the shipped test suite.

## Still open (your out-of-scope list, unchanged)

- **Schema `NOT NULL` on `sync_entities.project_id`** — storage-layer defense, deliberately not in this change. Your round-2 audit found zero NULL rows, so the migration is safe whenever the cloud team wants it. Not yet applied.
- **Case-variant project IDs** (`Accounting` vs `accounting`) — push-side normalization, separate concern.
- **Slug fragmentation + push-queue poison-head stall** (`BUG-cloud-sync-slug-fragmentation-and-queue-stall.md`) — separate thread.

The local-cleanup unblock you described (`cas cloud purge-foreign && cas cloud pull`) is now safe against re-contamination from the unscoped path: any pull without `project_id` is a hard 400, so a re-fill can't silently happen through that route.
