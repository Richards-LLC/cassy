---
to: Petra Stella Cloud team
from: Cassy CLI (cas-c117, EPIC cas-e0d9 — macOS clean-install field report)
date: 2026-08-18
priority: P1
status: client-side fix shipped; one server behaviour needs confirmation
---

# Project↔team registration only happens as a side effect of a non-empty team push

## What the user saw

On a clean macOS install (cas 2.72.0, schema v236, endpoint
`petra-stella-cloud.vercel.app`):

1. `cas cloud team set 2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb` → success.
2. `cas cloud sync` → `✓ Push complete`, `✓ Pull complete`, exit 0, 0 entries.
3. `cas cloud team-memories --full` → "This project hasn't been synced to the
   team yet. Run `cas cloud sync` while a team is configured" — the exact
   command just run.

Reproduced identically in a plain folder, with `team auto on`, with
`cas cloud project set <canonical-id>`, and in a real clone of
`github.com/Richards-LLC/gabber-studio`. `cas doctor` healthy throughout.

## Client-side root cause (fixed here)

The server learns about a project only through the `project_canonical_id`
field of `POST /api/teams/{teamId}/sync/push` (the route upserts the project
row — `INSERT … ON CONFLICT DO NOTHING`, previously reported in
`completed/BUG-team-memories-never-populate.md`).

The CLI only issued that POST when the team sync queue had rows:
`CloudSyncer::push_team` returns early on an empty queue
(`cas-cli/src/cloud/syncer/team_push.rs`). Team rows are enqueued at write
time, so a clean install — nothing written since the team was configured —
sent no team push at all, and the project was never registered. Everything
downstream (team memories, team pull scoping) then silently had nothing to
work with, while the sync reported success.

**Fixed client-side (cas-c117):** `cas cloud sync` now performs an explicit,
verified registration *before* it reports any success:

1. `GET /api/teams/{teamId}/projects` — already registered?
2. If not: `POST /api/teams/{teamId}/sync/push` with
   `{"entries": [], "project_canonical_id": "<canonical>", "git_remote": "<normalized>", "client_version": …}`
   (gzip, same shape as a normal push, no rows).
3. `GET /api/teams/{teamId}/projects` again — the registration counts as done
   only when the server itself lists the project.

If step 3 still does not list the project, the CLI exits non-zero and prints
the exact interaction instead of a green checkmark. A confirmed registration
is cached locally so steady-state syncs cost no extra round-trip
(`cas cloud sync --full` re-verifies).

## What we need confirmed on the server

**Does `POST /api/teams/{teamId}/sync/push` register the project when the
payload carries `project_canonical_id` but no entity rows?**

The client now depends on that being true (it is how a machine with nothing
queued registers at all). We could not verify against production from the
CLI repo — no credentials on the build machine, and the server lives in
`petra-stella-cloud`, which is not checked out here.

- If the route already upserts the project before it looks at the entity
  arrays: nothing to do, the client fix is complete.
- If the route short-circuits on an empty payload (e.g. returns early when
  every entity array is empty), the entity-less registration write is a no-op
  and users on a clean install will now get a loud, accurate failure instead
  of a silent one — better, but still blocked. In that case please either
  register before the empty-payload check, or expose a dedicated endpoint,
  e.g. `POST /api/teams/{teamId}/projects` with
  `{"canonical_id": …, "git_remote": …}` returning the project row. We will
  switch the client to it.

The exact request the client sends for registration:

```
POST /api/teams/{teamId}/sync/push
Authorization: Bearer <token>
Content-Type: application/json
Content-Encoding: gzip
{"entries":[],"project_canonical_id":"github.com/richards-llc/gabber-studio","git_remote":"github.com/richards-llc/gabber-studio","client_version":"…","client_build":"…"}
```

Expected: 2xx, and the project appears in `GET /api/teams/{teamId}/projects`.

## Related

- `completed/BUG-team-memories-never-populate.md` — the original report that
  documented the server's auto-register-on-push behaviour.
- `RESPONSE-cloud-open-decisions-and-git-remote-spec.md` §5 — `git_remote`
  normalization used by the registration payload.
