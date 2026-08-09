# Request: make cloud entity identity project-scoped

**Observed:** 2026-08-09 during `cas-1037` Mac-handoff parity repair  
**Boundary:** `petra-stella-cloud`; no server mutation was attempted here

## Problem

CAS Cloud identity behavior is inconsistent across short entry and task IDs
even when pulls and pushes are project-scoped. A push can resolve the correct
canonical project and still return HTTP 200 with `skipped: 1` because another
project already owns the same short ID. Direct entry lookup is also global,
while some project-scoped entry copies appear later. This makes a clean
project-scoped handoff impossible without risking another project's data.

A live team push for this machine resolved and echoed canonical project ID
`cas-src`, while the 44 missing local task rows were still skipped. Direct ID
lookups and scoped pulls classified the residual as follows:

- 20 task IDs resolve to the same title and creation instant, but the rows are
  stored under another or legacy-unscoped project binding and are absent from
  the `cas-src` pull.
- 24 task IDs resolve to genuinely different tasks owned by other projects.
- 11 still-missing entry IDs resolve to genuinely different entries owned by
  other projects through the direct lookup path.

Three entries that direct lookup resolved to foreign content became visible as
correct local-equivalent team-scoped copies at 13:34 and disappeared again by
13:38 while the queue remained empty. A fourth personal entry visible at 13:26
also disappeared. Five consecutive pulls from 13:39:11 through 13:39:36 then
stabilized at the 11-entry residual. This shows entry lookup, skip reporting,
or visibility is not consistent enough for a single-snapshot backfill receipt.
The task path remained stable and did not converge.

Known conflicting scoped projects included `ozer`, `gabber-studio`,
`domdms`, `pantheon`, `rocketship-template`, and
`petra-stella-cloud`; some same-identity legacy task rows were not visible
through the known project-scoped pulls. The complete row-level evidence is in
`docs/purge-receipts/cas-1037-operator-review-residual.tsv`.

## 2026-08-09 queue-wedge update

The defect recurred immediately for current work, and this incident provides
an independently reproducible safety case rather than a historical residual:

- A fresh `cas cloud sync` from `cas-src` returned `skipped: 29` for all 29
  team task upserts while echoing the canonical project `cas-src`. The local
  queue remained at 60 rows: 29 matching personal upserts, 29 team upserts,
  one personal-only task update, and one team-only tombstone.
- The matching personal rows also remained queued. Their skip is currently
  tracing-only, so the team response plus unchanged queue is the observable
  client evidence that the server rejected both scopes.
- Read-only comparison against `gabber-studio` found two concrete different
  owners for queued IDs: `cas-2627` is the CAS delivery-state task locally but
  is “Follow-up cleanup from cas-8d1e — dead Array.isArray guard + mount-test
  infra gap” in gabber-studio; `cas-ed01` is CAS cloud-pull work locally but
  is “Iterate chat shows user message twice — remove double-append” there.

These are genuine current local updates, including `cas-4fa4`, `cas-bfee`,
and `cas-3d85`; they are not safe candidates for client-side dequeue. In
particular, `cas cloud purge-foreign` works by deleting local content then
re-pulling. Its refusal on these unpushed rows is therefore data protection,
not a queue-health problem to override.

## Requested repair

1. Make entry and task identity composite on `(canonical_project_id, short_id)`
   across storage, lookup, upsert, delete, pull, and every uniqueness index.
   A correct `cas-src` upsert must coexist with the two foreign IDs above.
2. Provide an evidence-preserving migration and dry-run report for legacy
   globally-keyed rows. It must distinguish a provably identical row that may
   be safely re-bound from a conflicting row that requires a new
   project-scoped copy; neither path may overwrite a foreign row. This is
   also the safe route for the 20 same-identity task rows to `cas-src`.
3. Make direct entity lookup project-aware; an unscoped short-ID lookup must
   not silently select a row from another project.
4. Keep returning the resolved canonical project ID and structured per-entity
   results. Replace aggregate-only `skipped: N` with an itemized rejected-row
   list, for example `{id, entity_type, reason:"project_identity_collision",
   existing_canonical_id}`. Aggregate counts may remain for compatibility, but
   clients need the stable reason and ID to retain the correct queue row and
   present actionable diagnostics without guessing which records were lost.
5. Add a migration/dry-run report that lists ambiguous rows for operator
   review before any destructive change.

## Acceptance proof

After migration, pushing the 55 residual rows under canonical project
`cas-src` should insert or update only that project's copies. A scoped pull
must then match the local ID sets while the currently conflicting foreign rows
remain byte-for-byte unchanged in their own projects.
