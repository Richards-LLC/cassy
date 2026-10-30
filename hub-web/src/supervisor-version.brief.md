# Supervisor version badges

## Single idea
The installed Cassy version does not tell Daniel which live supervisor still needs its MCP connection refreshed; put the running version beside each supervisor's identity.

## Hero form
Annotated comparison: the existing supervisor list is the comparison, with a quiet version on current rows and a warning arrow on stale rows, next to the exact session that needs attention.

## Emotional register
Quiet, precise — existing Glass chrome and mono metadata keep version information beneath project identity; warning ink and an arrow reserve emphasis for an actual installed upgrade.

## Distinctive move
The version travels with the supervisor codename, so two supervisors on one machine can visibly run different versions without borrowing the machine's version label.

## Deliberately omitted
No new status panel or release polling: reuse fingerprinted Factory MCP version observations and the installed binary comparison. Unknown or offline observations show no badge; no guessed version from the hub binary. Latest-release comparison is optional and omitted to keep the actionable restart state tied to the installed binary.

## Design and implementation contract

design: hub-web/DESIGN.md@97d9f6b70

LocalSessionReadModel owns a short-lived per-project snapshot from factory_binary_skew. HubSession exposes optional running MCP and installed versions; the browser derives badge state without mutable duplicate state. Conversation rows own rendering and accessible naming. Existing mono-xs, ink-mid, state-warn, space-1 and radius-card tokens style the badge. No motion or new network request. Unknown, ambiguous, dormant, ended and offline observations do not claim a version. Numeric comparison reuses machine-version's stable-release comparison.

Acceptance: one table-style row-rendering unit test for current/stale/unknown/offline states, one extension of HUB-J3's existing machine-version journey for Linux/Claude and macOS/Codex session observations, backend projection evidence, typecheck, affected journeys, candidate-backed before/after capture and visual critique. No new dependency or image; generated assets stay outside the commit.

## Critique
Pending real candidate render and visual checks. Required craft floor: 4/5.
