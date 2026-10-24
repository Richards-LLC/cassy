# Historical managed replay: consumer-local audit (#1274, cas-35ab)

Prowl retains both incident inbox rows. Routine automatic intake excludes
these seven-day-old rows, but this is **not retroactive containment** of the
stored copies. No suppression, deletion, credential rotation or transcript
remediation was performed. Soundwave and transcript occurrence correlation
remain unverified. **Owner rotation confirmation: pending.**

## Authority and method

Supervisor decision CAS #546083 authorized read-only metadata queries on
prowl, directed soundwave to be marked NOT EXERCISED with owner-run queries,
and requested the remaining state be posted on GitHub without waiting for
credential-owner confirmation. Daniel owns the backend environment; Ben
owns the matching ManyChat flow. The supervisor is relaying the coordinated
rotation request; this audit has no owner confirmation or model-action proof.

Host: prowl, macOS; installed runtime: `cas 3.77.0 (2020ff1 2026-10-24)`.
Source inspected: `0bbeeab4d37ce541459ce2e4d56ea795b2b4339a`.
Store: `/Users/pippenz/Petrastella/gabber-studio/.cas/cas.db`.
SQLite used `mode=ro` and `query_only=ON`, with an authorizer allowing only
the named metadata columns. Preparation of reads of `prompt`, `content` and
`metadata` was deliberately denied before any cell could be fetched.
No message/body columns were selected, searched, hashed or copied. No DB
snapshot, inbox drain, Slack fetch, Cloud request or production write ran.

Private evidence lives in the assigned `cas-35ab/proof/` artifact directory:
`audit.sql`, `audit.py`, `before.json`, `after.json`, and
`transcript-metadata.json`. The helper is an evidence capture, not a new runtime
command. Its initial SQL-comment splitting error failed before capture; the
corrected final query set produced the receipts below.

## Before and after

The same ten queries ran at **2026-10-24 16:19:04.016024Z** and
**16:19:19.330666Z**. All query results were identical. These are observations
around a read-only audit, not an intervention comparison.

| Metadata query | Before rows | After rows |
| --- | ---: | ---: |
| Named inbox rows 2565530 / 2565537 | 2 | 2 |
| Channel intake candidates, Oct 17 18:00–19:30Z | 30 | 30 |
| Candidate aggregate | 1 | 1 |
| Named-row recipient-seen receipts | 3 | 3 |
| Named-row transport receipts | 2 | 2 |
| Quarantine records matching named row IDs | 0 | 0 |
| Retained agent mapping for wise-shark-78 | 0 | 0 |
| Recording-agent mapping for wise-shark-78 | 0 | 0 |
| Indexed prompt metadata, 18:20–18:35Z | 0 | 0 |
| Same-day session metadata overlapping 18:27:39Z | 2 | 2 |

The 30 intake candidates are **not an exposure count**: source timestamps
and activation are not separate columns in these legacy queue rows. Of these,
27 belong to `gabber-studio-young-crane-31`. All 30 are processed; five lack an
ACK; none has a terminal dropped/suppressed/abandoned stage; zero fall within
the current 24-hour automatic-intake cutoff. Classifying every candidate as
historical would require the Cloud occurrence-to-delivery metadata mapping.

| Row | Transport time (2026-10-17 UTC) | Recipient evidence | State, unchanged |
| --- | --- | --- | --- |
| 2565530 | 18:26:32.206271 | `supervisor`: transport-claimed receipt; no ACK | delivered; no explicit quarantine |
| 2565537 | 18:27:39.691003 | `supervisor` and `wise-shark-78`: inbox-poll receipts; explicit ACK 18:27:46.268318 | confirmed; no explicit quarantine |

Both target `supervisor` in `gabber-studio-young-crane-31`. Their delivery keys
are respectively `violet-delivery:5ed73773-265e-4eb2-9609-db82fbc71f0a` and
`violet-delivery:06c90c4d-7854-4126-a7f9-3c3e07e38862`.
The link between occurrence `1791927632.856139` and row 2565537 comes from the
existing incident record, not a new body inspection. Row 2565530's historical
classification likewise comes from that record; its current transport receipt
does not establish model handling.

The two candidate sessions are `76bfeb48-26a9-4ec4-8698-f99a7a722381` and
`51d2473c-3bc2-42e7-adc3-f880f4efe0a7`. Filename glob/stat under seven
`/Users/pippenz/.claude*/projects` roots found one transcript file each,
last modified respectively 2026-10-17 15:34:56.733357Z and
18:35:18.504336Z. No transcript contents were opened. Neither session is
proven to contain the occurrence, and no occurrence-level suppression flag
is available from this metadata. Zero indexed prompt rows does not prove
absence from raw transcripts. **Transcript containment: NOT VERIFIED.**

## Source confirmation and containment limits

These paths were traced before any delivery-file changes:

- `cas-cli/src/ui/factory/daemon/runtime/violet_reader.rs:133`: classifies
  `message_ts` against activation. Lines 375–377 quarantine withheld claims
  before ACK; lines 549–560 guard fetch/typing. Lines 725–733 enqueue eligible
  text into the local prompt queue. Lines 750–788 write metadata-only
  quarantine with 14-day retention. This protects future admission; it does
  not alter previously stored inbox rows or transcripts.
- `cas-cli/src/mcp/tools/service/agent_search_system/message.rs:3258`:
  `inbox_poll` drains bodies and records receipts, so it was unsuitable for
  this audit. Lines 3341–3350 explicitly handle named reads before stale
  quarantine.
- `crates/cas-store/src/prompt_queue_store.rs:87`: automatic stale cutoff is
  24 hours. Lines 4934–4973 apply stale/terminal/recipient/session filters.
  Lines 9459–9483 let a matching recipient/session explicitly request a
  retained row despite age or ACK. Thus age filtering and an ACK are not
  secure suppression of stored copies. `mark_suppressed` at line 3258 is an
  internal noise-suppression primitive, not a supported incident-control
  operation for an already delivered transcript; this audit did not misuse it
  or forge acknowledgement.

Prior receipts were read from the canonical task notes:

- `cas-b94d`, Oct 17 19:26: Cloud partial server hold at production
  `7bc56022375ee1c8db3a73e10fa03d9825013e53`, receipt `art-56aecd65`:
  three holds enabled 19:24:19–20Z; 24 historical occurrences / 48 already
  consumed deliveries; three audit actions / 24 suppression rows. The note
  explicitly excludes local queued bodies/context. This is a **historical
  receipt**, not a fresh Cloud verification. Source admission fixes shipped
  as `f448e0d5a` and `f8ab35a26` in v3.65.
- `cas-30c0`: `84c27111d7bd037ec47d09eb3aea121cdde8089c` sorts each recovered
  page by numeric message timestamp, preserving fresh-first/idempotency;
  shipped in v3.73. Cross-page ordering belonged to Cloud `cas-3e64`.
  Sorting does not remove old copies.

**Remaining boundary:** consumer owners need a supported, reversible
occurrence-specific hold for existing inbox copies and a retention/access
decision for raw transcripts, preserving the original audit. Cloud/Violet
must supply the metadata-only occurrence → consumer delivery → local dedupe
mapping to classify the full set. No global containment or full exposure
count is established. Existing #1274 remains open for that work.

## Soundwave: NOT EXERCISED

This session cannot reach soundwave. An authorized owner can run this exact
read-only projection against that consumer's local Gabber DB, saving metadata
output before and after any separately authorized containment. Do not reuse
prowl's numeric row IDs on another consumer. Do not run a normal inbox poll or
read transcript bodies to fill the missing mapping.

```sql
SELECT id, source, target, created_at, processed_at, acked_at,
       transport_delivered_at, highest_stage, dedupe_key, factory_session
FROM prompt_queue
WHERE source = 'violet-activity:C09FCTHCQ2U'
  AND created_at >= '2026-10-17T18:00'
  AND created_at < '2026-10-17T19:30'
ORDER BY id;

SELECT seen.prompt_id, seen.recipient, seen.seen_at, seen.source,
       seen.poll_settled, seen.delivered_at
FROM prompt_queue_recipient_seen AS seen
JOIN prompt_queue AS q ON q.id = seen.prompt_id
WHERE q.source = 'violet-activity:C09FCTHCQ2U'
  AND q.created_at >= '2026-10-17T18:00'
  AND q.created_at < '2026-10-17T19:30'
ORDER BY seen.prompt_id, seen.recipient;
```

Invocation (replace the path locally; no credentials required):

```sh
sqlite3 -readonly /absolute/path/to/gabber-studio/.cas/cas.db -json < audit.sql
```

The time window scopes the known incident intake, not all historical exposure.
Use Cloud's metadata mapping to locate any consumer deliveries outside it;
absence in this window must not be reported as zero exposure. Soundwave
transcript correlation and containment require its owner's metadata receipt.

## Acceptance disposition

| Criterion | Disposition |
| --- | --- |
| Metadata-only local before/after | PASS: identical projected results, row counts preserved |
| Determine whether copies are contained | PASS as an audit finding: explicit local containment NOT VERIFIED; no remediation claimed |
| Soundwave audit | NOT EXERCISED: unreachable from this session; owner query above |
| Transcript occurrence audit | NOT VERIFIED: metadata-only occurrence/session mapping missing |
| Credential remediation | Owner rotation confirmation: pending; Daniel/Ben, relayed by supervisor |
| Preserve audit without secret values | PASS: read-only metadata projections; original records retained |
| GitHub remaining-state comment | Receipt recorded in task notes and private proof artifact |

This is an evidence-only incident follow-up. No product code, schema, UI or
journey changed; no Rust build, browser run or new code tests were warranted.
