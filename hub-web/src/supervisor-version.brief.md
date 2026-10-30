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

Source e26440e4f206a081794f3a92b607d0181052dead was rendered through supervisor-built QA-only candidate 3feb3530b852c5f345e3354b0e5026189694897e, whose embedded Web receipt ab64d84d matches source. Task cas-8125's qa/critique.md, qa/bundle.json and qa/pairs/pairs.json hold the full record. Scored by fierce-heron-55 on 2026-10-30.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Mono version follows its supervisor codename inside the existing Glass project list, separate from machine identity. |
| Fit | 5 | A warning arrow identifies the exact old process; current versions remain quiet and unknown versions remain absent. |
| Hierarchy | 4 | Project and machine lead; version metadata steps down. Light/dark desktop and phone captures retain this order. |
| Craft | 4 | Strict visual QA PASS across eight renders at 1280×800 and 390×844: zero findings, zero allowlisted issues, no clipping or overflow. |
| Accessibility | 4 | Row accessible names carry version/reconnect guidance; keyboard focus and forced colors, reduced motion and increased contrast checks pass on the real candidate. |

The craft floor holds. Four actual candidate before/after pairs use identical data, scheme and viewport; protocol-double inputs are disclosed. The final source passes 216 producer-selected journey parts, 1,573 web unit tests, typecheck and 190 affected Rust tests. macOS host evidence is owned by prowl and remains separate from Linux captures of a Mac-labelled protocol-double row.

Compared with the task-start baseline, the final merged candidate adds 295 gzip bytes to app.js and 219 to app.css, including imported epic changes. Existing entry assets already exceed the skill's default size budgets; this change adds no dependency, media, animation or release request. LCP and INP were not measured.
