# Slack draft — team-scoping ergonomics (2026-07-06)

Channel: #cas-internal (`C0B44GUKDK2`) — two top-level posts.

---

## Post 1 — User

Setting up team sync used to mean hunting down a raw team ID from a dashboard and pasting a UUID into a command — and if you skipped it, your project quietly synced to just you, with no hint anything was off. Now Cassy already knows your teams: link a project by name, or with no argument at all, and Cassy speaks up (once) if you're syncing solo while a team is available.

- `cas cloud team set petra-stella` just works — team names resolve automatically from your login. If you're only on one team, plain `cas cloud team set` links it with zero typing. Raw UUIDs still work.
- New `cas cloud team auto on | off | clear` — one command to make a project follow your default team, hard-block team sync, or go back to the default, instead of hand-editing a JSON file.
- One-time heads-up: if a project is syncing to personal scope while you're a member of a team, Cassy tells you once — with the exact command to link it — and never nags again.
- Your choice always wins: nothing ever switches a project to team scope on its own. The notice informs; only your command changes anything.

## Post 2 — Dev

Team scoping ran on raw UUIDs and hand-edited JSON: `team set` accepted only a UUID, `team_auto_promote` had no CLI at all, and a mis-scoped project failed silent. Now the whole flow runs off the identity we already cache from `/api/me` — and the informational path is concurrency-safe.

- `cas cloud team set <slug|uuid>` resolves slugs (and zero-arg single-team) against cached `teams[]` in `~/.cas/cloud.json`; `team default` shares the same resolver instead of duplicating the lookup. Unknown slugs error with the cached options + a login hint.
- `team_slug` is now persisted to project `cloud.json` on set (previously hardcoded `None`), so downstream display paths stop falling back to bare UUIDs. JSON output carries `team_slug`/`team_name`.
- `cas cloud team auto on|off|clear` writes the tri-state `team_auto_promote` (inherit / kill-switch / default-personal) and prints the effective team via the real resolution chain, warning when `on` still resolves to personal.
- One-time personal-scope notice (project resolves personal + user has a usable team) fires in both `cas cloud sync` and the embedded daemon's sync loop; suppression flag lives in project `cloud.json`.
- Hardening: the notice's flag write re-loads the config fresh and re-checks eligibility immediately before saving — a concurrent `team set`/`team auto on` can no longer be clobbered by a stale in-memory snapshot (regression-tested with a simulated concurrent team link).
- The privacy invariant is untouched: projects stay personal unless explicitly linked or opted in; no code path auto-mutates team scope.
