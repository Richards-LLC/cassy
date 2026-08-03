---
from: Gabber Studio team (Daniel)
date: 2026-06-25
priority: P2
---

> Migrated to GitHub Issues: [#65](https://github.com/pippenz/cas/issues/65)

# Feature Request: ship a release-notes / Slack-announcement rubric + a CAS-managed CLAUDE.md breadcrumb to it

## What we need

CAS should **own a release-notes / Slack-announcement rubric** as a framework-level artifact (the announcement analog of what `codemap`/`project-overview`/the proposed `design-spec` are for their domains), and **breadcrumb to it from the CAS-managed `CLAUDE.md` block** so every CAS project picks it up automatically — including cas-src's own checked-in `CLAUDE.md`.

Prototype already built by hand in the Gabber repo:
- `docs/release-notes/RUBRIC.md` — the rubric (target shape).
- `CLAUDE.md` (now **tracked** — removed from `.gitignore`) — has a "Release notes — REQUIRED on every staging/main merge" section linking to the rubric.

## The rule

**Every PR merged to `staging` or `main` must be announced in Slack following the rubric.** Today that's a per-project convention only the Gabber `CLAUDE.md` states; it should be a CAS-managed expectation that travels with every project.

## The rubric (prototype)

Post to the project's release channel (Gabber = `#gabber-internal` `C09FCTHCQ2U`). **Two threads** — User, then Dev — each = a labeled punchy top-level + ONE threaded reply with **Was → Now** detail:
- **Top-level (one punch):** prefix with the deploy target — **`Staging`** (merged to staging) or **`Live on production`** (merged to main) — plus the perspective (User / Dev), then one plain-language sentence.
- **Reply:** the detail as Was → Now. User = plain language anyone understands; Dev = more technical, PR numbers OK (Dev thread only).

**Hard rules:** Was → Now per item; **no internal ticket labels (`cas-XXXX`)** in the copy; **no internal / agent / coordination / deploy drama** (describe the product, not the process — never mention agents, worktrees, "the deploy broke", retries, blame); plain language; honest revert acknowledgment.

## Proposed scope

1. **Ship the rubric** as a framework artifact CAS can drop into a project (like the `CAS:BEGIN/END` block) — a canonical `docs/release-notes/RUBRIC.md` template.
2. **Breadcrumb in the CAS-managed `CLAUDE.md` block:** add a short "Release notes: on every staging/main merge, announce in Slack per `docs/release-notes/RUBRIC.md`" line + link inside the `<!-- CAS:BEGIN ... CAS:END -->` section CAS already manages, so it propagates to all projects.
3. **Check it into cas-src's own `CLAUDE.md`** so the framework repo eats its own dog food.
4. (Optional) a `release-notes` skill: given a merge range, draft the user/dev/slack triplet per the rubric and post the two threads to the project's channel (top-level → capture `ts` → threaded reply, ×2).

## Why

- The rubric currently lives only in one project's hand-edited files; new projects won't inherit it.
- The "announce on every staging/main merge" expectation is easy to forget unless it's in the CAS-managed `CLAUDE.md` block.
- Same "document once, reuse everywhere, regenerate on drift" pattern `codemap` already proved — applied to release comms.

## Prior art / format

See the Gabber prototype: `docs/release-notes/RUBRIC.md` + the recent dated triplets `docs/release-notes/<date>-<topic>-{user,dev,slack}.md` (the `-slack.md` is the postable draft).
