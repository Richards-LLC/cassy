# Corrupted cloud task rows — 98 rows fail every team pull

**Date:** 2026-08-30 · **Author:** factory supervisor session 9bc9520d · **Status:** repair scheduled (EPIC cas-5697)

## Verdict

98 task rows in the team cloud carry `deliverables` serialized as a JSON **string** instead of the
`TaskDeliverables` struct. Every team pull on every cloud-linked project rejects all 98 and applies
partial results — 8 projects × 98 = 784 error lines in a single `cas update` run. The rows are
invisible to sync on every machine until repaired. Client fix is cas-6980; one-time cloud repair is
cas-9894; both are children of EPIC cas-5697 with workers assigned.

## Evidence

- Source log: `cas_update_20280830.txt` (cas-src repo root, `cas update` run 2026-08-30 10:48 local).
- Error shape: `task deserialize error (id=cas-XXXX): invalid type: string "{\"files_changed\":[]}", expected struct TaskDeliverables`.
- Warning per project: `⚠ Team pull encountered 98 error(s); partial results applied` — identical for
  all 8 cloud-linked projects (log lines 40, 160, 305, 427, 547, 667, 787, 907).
- ID extraction: `grep -oP 'id=\Kcas-[0-9a-f]+' cas_update_20280830.txt | sort -u` → 98 unique IDs.
- Local cross-reference: cas-src `.cas/cas.db` queried read-only 2026-08-30.

## Breakdown of the 98

| Class | Count | Disposition (cas-9894) |
|---|---:|---|
| Local **open** cas-src tasks — live work, cloud copy corrupted | 60 | Re-push clean local canonical |
| Local **closed** cas-src tasks — archive, cloud copy corrupted | 10 | Re-push clean local canonical |
| **Cloud-only tombstones** — contamination deleted locally 2026-08-30 (ozer cluster, cas-43a1, cas-b492, cas-e42d) | 28 | Delete cloud copy |

### Highest-impact row

`cas-9cf9` (P1, publish the 2026-08-25 harness diary thread — the sole remaining child of live EPIC
cas-abfc) is the only row whose stringified payload carries **real data**, not the trivial
`{"files_changed":[]}`: a `work_target` (repo_selector `project:cas-src`, target branch
`epic/epic-2026-08-25-harness-diary-sweep-…-cas-abfc`), `factory_branch_anchor` `fdf43b20`, and
`parked_branch` `factory/eager-heron-43`. Its cloud copy cannot sync to any machine, so the diary
publication task exists only on this host until repair.

Notable also in the 60 open: EPIC cas-abfc itself and cas-7657.

### The 60 open rows (P1 → P4)

cas-7657, cas-9cf9, cas-abfc (P1); cas-0101, cas-1b12, cas-2598, cas-3b29, cas-467a, cas-48e4,
cas-4c89, cas-5bc14, cas-63b9, cas-6c28, cas-7265, cas-74e3, cas-844b, cas-9886, cas-9d8a, cas-b8b1,
cas-c2b2, cas-c2f2, cas-c8a6, cas-ce30, cas-dae6, cas-dfa3 (P2); cas-046c, cas-0906, cas-1a95,
cas-1c31, cas-236d, cas-24d0, cas-269ab, cas-2929, cas-326e, cas-3316, cas-3843, cas-4cec, cas-563a,
cas-5eb9, cas-5f61, cas-6419, cas-7062, cas-7391, cas-746a, cas-74b9, cas-74c8, cas-8090, cas-84b3,
cas-9040, cas-939a, cas-96ab, cas-9fe6, cas-a5c0, cas-b352, cas-d4a0, cas-d6b9, cas-de46, cas-ec70,
cas-f163 (P3); cas-0f22 (P4).

### The 10 closed rows

cas-1833, cas-1ddf, cas-42a4, cas-8172, cas-9def, cas-a81f, cas-c40a, cas-cdcc, cas-d132, cas-eaf6.

### The 28 cloud-only tombstones

cas-082b, cas-0acb, cas-0b21, cas-1818, cas-29a6, cas-2cbb0, cas-3ac7, cas-43a1, cas-5548, cas-5f2e,
cas-71b7, cas-8b21, cas-8e9d, cas-9841, cas-9c3a, cas-9f5e, cas-a1e0, cas-a2e6, cas-adb8, cas-b492,
cas-bcfe, cas-c97a, cas-dbf9, cas-dcbc, cas-e42d, cas-e446, cas-ea8d, cas-f179.

## Root cause

The cas-02a7 cross-DB contamination cleanup (2026-08-29) relocated ~789 rows between project DBs.
Its writer serialized the `deliverables` field by JSON-encoding an already-encoded value, producing a
string where the sync contract expects a struct (tracked as **cas-6980**, filed before this run and
now carrying this field evidence). The corrupted values reached the cloud through that session's
pushes; the current client refuses them on pull, correctly, but with row-level silence about the
consequence: partial pulls forever.

## Blast radius and what it is not

- **Not data loss on this host:** all 70 local rows are intact here; only their **cloud** copies are bad.
- **Cross-machine loss until repair:** any other machine pulling the team project gets none of the 98 —
  including live P1 work (cas-9cf9, cas-abfc).
- **A silver lining:** the 28 corrupted tombstones never resurrected our deleted contamination —
  they fail to deserialize, so pulls skip them.

## Repair plan (scheduled)

1. **cas-6980** (worker assigned): client tolerates/repairs string-encoded deliverables on pull
   (double-decode), and the writer that stringified is fixed at the source.
2. **cas-9894** (blocked by cas-6980): re-push the 70 clean local canonicals; delete the 28 cloud-only
   tombstones; prove team pulls report 0 errors from at least 2 projects, with per-ID before/after receipts.

## Provenance

All counts derive from: (a) `cas_update_20280830.txt` as shipped in the cas-src repo root, unmodified;
(b) read-only SQLite queries against `/home/pippenz/Petrastella/cas-src/.cas/cas.db` on 2026-08-30
(98-ID `IN` list; status grouping `open=60, closed=10`; set difference for the 28). Reproduction
commands are inline above.
