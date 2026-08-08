---
task: cas-a41d
epic: cas-e000
date: 2026-08-07
type: spike (recon, no code changes)
base: origin/main @ 5d0419df27acb4977b32ba0c6998914d8a8c6717
constraints: read-only; zero live-store mutations; no sync runs against real stores
---

# Server-side sync reality: what the cloud actually supports today

Recon for the cas-e000 redesign. Four questions, answered with client-side `file:line`
receipts against `5d0419df`. Anything the client cannot observe is marked **UNKNOWN** or
**SERVER-ASSERTED**.

## Evidence classes used in this document

| Marker | Meaning |
|---|---|
| **VERIFIED** | Read in this repo at the cited `file:line` on base `5d0419df`. |
| **SERVER-ASSERTED** | Stated by the petra-stella-cloud team in `docs/requests/completed/RESPONSE-cloud-knowledge-sync-and-embeddings.md` (789 lines, on main). Their implementation claim, with their own citations into *their* repo. Credible and detailed, but not reproducible from cas-src. |
| **UNKNOWN** | Cannot be determined from this repo, and not covered by their response. |

The distinction matters for the redesign: every "the server enforces X" below is
SERVER-ASSERTED unless the client independently re-checks it. Where the client does *not*
re-check, the server is the **only** line of defence, and a server regression is silent.

---

## Q1. Which endpoints does the client hit, with what scoping, and what does the server enforce vs trust?

### 1.1 Full endpoint inventory (sync-relevant)

| # | Method + path | Built at | Scoping the client sends |
|---|---|---|---|
| 1 | `POST /api/sync/push` — queue-driven personal push | `cloud/syncer/push.rs:457` | body `project_canonical_id` (**required**, `:469-474`), optional `team_id` (`:465-467`), `client_version`/`client_build` (`:477`) |
| 2 | `POST /api/sync/push` — session push | `cloud/syncer/push.rs:195` | same three (`:201-203`, `:206-211`, `:214`) |
| 3 | `POST /api/sync/push` — **snapshot push (the one `cas cloud sync` actually uses)** | `cli/cloud.rs:1741` | `project_canonical_id` (`:1822-1825`), `client_version`/`client_build` (`:1827-1834`), optional `team_id` (`:1836-1838`) |
| 4 | `GET /api/sync/pull` — personal entity pull | `cloud/syncer/pull.rs:23` + `:38-62`, called `:232` | `project_id=` (**always**, `:59`), `since=` (`:229-231`). **No `types=`.** |
| 5 | `GET /api/sync/pull` — knowledge-page pull | `cloud/syncer/knowledge.rs:221-229` | `types=knowledge_pages` (`:221`), `since=` (`:222-224`), `project_id=` via the same builder. **No `team_id=`.** |
| 6 | `DELETE /api/sync/{entity_type}/{id}` | `cloud/syncer/push.rs:360-364` | path only — **no project or team scope at all** |
| 7 | `GET /api/sync/status` | `cli/cloud.rs:1226` | none |
| 8 | `POST /api/teams/{teamId}/sync/push` | `cloud/syncer/team_push.rs:212` | `project_canonical_id` (`:218-221`), **`git_remote`** when resolvable (`:222-224`), client version (`:225`) |
| 9 | `GET /api/teams/{teamId}/sync/pull` | `cloud/syncer/pull.rs:909-920` | `project_id=` (`:917`), `since=` (`:914-916`) |
| 10 | `DELETE /api/teams/{teamId}/sync/{type}/{id}` | `cloud/syncer/team_push.rs:522` | path only |
| 11 | `POST /api/embeddings` | `cloud/embeddings.rs:174` | none — `{model, input[]}` only (`:175-178`) |
| 12 | `GET /api/me` | `cloud/me.rs:83` | none |
| 13 | `GET /api/teams/{teamId}/tasks/{taskId}/comments` | `cloud/comments.rs:104` | path only |
| 14 | `/api/agents/*` (register, heartbeat, claim, release, renew, lock, locks) | `cloud/coordinator.rs:125-458` | path only — outside sync scope, listed for completeness |

**Auth on every one of them is a single per-account bearer token.** SERVER-ASSERTED: API
keys are per-user (`psc_k{n}_{hex}`), there is no org- or team-scoped token, and every
scoping decision is derived from that one identity (their §Q1). So `team_id` and
`project_id` on the wire are **claims by the client**, not credentials.

### 1.2 What the client enforces on itself

- **Pull fails closed without a project scope.** `build_scoped_pull_url_with` returns
  `Err("Cannot pull: not inside a CAS project directory")` when the canonical id is
  unresolvable (`pull.rs:55-57`), and `project_id=` is appended unconditionally
  (`pull.rs:59`, `/`→`%2F`). There is no code path that issues an unscoped
  `/api/sync/pull`. `PULL_PATH` is deliberately the only place the literal is written
  (`pull.rs:17-23`) so no caller can bypass the builder.
- **Push fails closed the same way** — `push.rs:206-211` and `push.rs:469-474` both
  hard-error rather than push without `project_canonical_id`; the snapshot path does the
  same at `cli/cloud.rs:1771-1772`.
- **A slug-rehome guard** refuses the snapshot push when the pinned project slug changed
  since the last successful push (`cli/cloud.rs:1779-1794`), best-effort/fail-open.

### 1.3 What the client trusts the server on — and where it re-checks

| Concern | Client re-check? | Receipt |
|---|---|---|
| Entity pull returns only in-scope rows | **Yes** — `entity_matches_project` on every row of every type | `pull.rs:73-125`, called at `:259, :284, :319, :344, :377, :406, :427, :451, :479` |
| Team pull returns only in-scope rows | **Yes** — same filter | `pull.rs:956, :981, :1006, :1031` |
| **Knowledge pull returns only in-scope rows** | **NO** | `knowledge.rs:228` discards the resolved id (`let (url, _project_id) =`); the apply loop `:257-273` never calls the filter |
| Push rows the server silently dropped | **Yes, conservatively** — a non-zero `skipped` count leaves the whole sub-batch unmarked for retry | `push.rs:324-345` |
| **Knowledge-push rows the server dropped** | **NO** — `push_sub_batch`'s response is discarded with `?;` | `knowledge.rs:196` |
| Embedding vector count matches input count | **Yes** | `embeddings.rs:211-217` |
| Embedding vector is non-zero / right dimension | **Yes** | `embeddings.rs:506-513` |
| `project_id` echoed on each row is the one we asked for | Implicitly, via byte-exact `==` | `pull.rs:106` |

**The load-bearing consequence** (also flagged by the cloud team, their §7.3-2b): because the
re-check at `pull.rs:106` is byte-exact and drops on mismatch with only a `eprintln!`
warning, any server change that echoes a *differently-cased or differently-derived*
`project_id` than the client sent silently drops **100%** of rows. That is a total blackout
presenting as a successful, quiet sync. It is a hard constraint on both sides.

### 1.4 What the server enforces (SERVER-ASSERTED, not verifiable here)

- `project_id` mandatory on both pull routes, `400` before any query runs; every
  row-returning query carries an equality filter on it; `project_id` echoed per record
  (their §7.3-1).
- `sync_entities.project_id` is now `NOT NULL` in production (migration 0013) (their §7.3-2a).
- **No server-side case normalization** — matching stays byte-exact, deliberately, because
  normalizing would trip `pull.rs:106` and blank an entire account (their §7.3-2b).
- Team push resolves the project **remote-first** (`git_remote` beats the sent slug); the
  **personal** push path does not — it stores the sent string verbatim (their §7.3-3).
  This is the asymmetry cas-7719 exists to close.
- Rate limits per account, sliding window: push 300/60s, pull 120/60s, embeddings 120/60s;
  `429` + `Retry-After`. Body caps 4 MB compressed / 20 MB decompressed → `413` (their §Q4).
- Conflict resolution uses the `updated_at` **carried in the record**, never arrival time
  (their §8).

**UNKNOWN:** whether the server validates that a caller may claim the `team_id` it sends on
a personal push (`push.rs:465-467` sends it as a plain body field). Their §Q1 says team
visibility is derived from live `team_members` membership on *read*, which implies the claim
is not load-bearing on write — but the write-side check is not stated and cannot be tested
from here.

**UNKNOWN:** whether `DELETE /api/sync/{entity_type}/{id}` (#6) is project-scoped. It carries
no scope at all from the client. SERVER-ASSERTED (their §Q7): it is scoped `user_id = caller`
and accepts **any** `entity_type` string without validation, including `knowledge_page`.

### 1.5 A real client-side gap worth naming

`max_payload_bytes` defaults to **5 MB uncompressed** (`syncer/mod.rs:184, :196`), sized
against a server cap that is **4 MB compressed / 20 MB decompressed**. Gzip normally closes
the gap, but the two numbers were never reconciled and neither side knows about the other's.
The snapshot path doesn't use it at all — it chunks by a fixed `BATCH_SIZE: usize = 50`
(`cli/cloud.rs:1744`), i.e. by *item count*, so a run of large entries can exceed the cap
regardless.

---

## Q2. Does any server surface exist for knowledge pages or embeddings?

**Yes to both — this is the question the recon most changes.** cas-6e38 requested a spec;
cas-1ac6 built capability-gated client pieces; the cloud team then **shipped the server half**
and documented it in `docs/requests/completed/RESPONSE-cloud-knowledge-sync-and-embeddings.md`.

### 2.1 Knowledge pages — SHIPPED server-side

SERVER-ASSERTED, from their "What shipped" table:

- `knowledge_pages` accepted on push, stored as `entity_type = 'knowledge_page'` in
  `sync_entities` (JSONB payload; `user_id`/`team_id`/`project_id` are real columns and are
  authoritative).
- The **§3.1 locked-page guard is implemented server-side**, and implemented *correctly* in a
  way we did not specify: because `sync_entities` is keyed `(user_id, entity_type, id)`, a
  teammate's copy is a **different row**, so the guard queries sibling rows sharing
  `(id, project_id, team_id)` rather than checking the pusher's own row.
- Lock refusals are reported as `skipped.knowledge_pages` in the push response — and
  **counts lock refusals only**, not benign last-writer-wins no-ops.
- Pull returns pages under exactly the `knowledge_pages` key; visibility is your own pages at
  any `share` ∪ teammates' `share:"team"` pages in teams you belong to, both halves filtered
  by `project_id`.
- Per-user row dedupe: `locked` wins, else newest `updated_at`, else `user_id` ascending.
- `since` on the knowledge path is **lenient** (`>= since - 5 min`); a malformed `since` is
  ignored rather than `400`. The generic pull keeps strict `updated_at > since`.
- `types=` is now honoured: omitting it returns the legacy envelope **without** a
  `knowledge_pages` key. Our generic pull sends no `types=` (`pull.rs:228-232`), so it never
  receives pages — correct by accident, and worth pinning as intentional.
- Bodies are stored as sent, never parsed/re-serialized, with a 39-case byte-identity
  regression lock that they **mutation-tested** (20/39 went red under an injected
  `.trim()`). `rel_path` is never recomputed.

### 2.2 Embeddings — SHIPPED and LIVE in production

SERVER-ASSERTED, verified by them against production by two people independently:

| | |
|---|---|
| Endpoint | `POST /api/embeddings`, live |
| Wire model | `cas-embed-v1` → OpenAI `text-embedding-3-large` @ **1024** dims |
| Normalization | unit L2, passed through verbatim; measured 0.999671 / 0.999738 (±3e-4) |
| Response shape | **flat only** — `{"embeddings": [[...]]}`, exactly one key |
| Persistence | **none** — no vectors, no input text stored or logged |
| Errors | count/dim/zero-vector mismatches are `502`, never a short `200`; unknown model `400`; >32 inputs `400`; rate limit `429` + `Retry-After` |

**Client-side match, VERIFIED:** `DEFAULT_EMBEDDING_MODEL = "cas-embed-v1"`
(`embeddings.rs:53`) and `DEFAULT_EMBEDDING_DIMS = 1024` (`:58`) are exactly the shipped
contract. They are **compile-time constants** baked in by `from_config` (`:126-127`), so a
model rename is a client release, not a config edit — which the cloud team has explicitly
committed to honouring.

**Two concrete client cleanups this unlocks:**

1. `parse_embedding_response` (`embeddings.rs:220-236`) can drop its `data[].embedding`
   branch — they will never emit it. Low urgency, zero cost to keep.
2. **A latent 400-shaped landmine.** `embed_pending_pages` sends **every** page it fetched in
   **one** `embed_batch` call (`embeddings.rs:490-503`) — there is no internal chunking. The
   server hard-caps input at 32. The only production caller passes a hardcoded literal `32`
   (`cli/cloud.rs:1171`) — while `DEFAULT_EMBED_BATCH = 32` exists at `embeddings.rs:62` and
   is **not used at that call site**. The invariant is currently held by a duplicated magic
   number in a different file from the constant that documents it. Any future caller passing
   `64` gets a permanent `400` and zero pages ever embedded, silently (the error is swallowed
   as a `tracing::warn!` at `cli/cloud.rs:1177`).

### 2.3 What is still genuinely missing server-side

- **Tombstones for knowledge pages: none.** SERVER-ASSERTED and agreed by them as a real gap
  (their §Q8/§11.4). The generic `DELETE` route accepts `knowledge_page` but hard-deletes only
  the caller's row, records no tombstone, and — because pull dedupes across per-account rows —
  the next pull can re-deliver the page from a teammate's row. Do not build delete on it.
- **`team_id` on the knowledge pull: not sent by the client** (`knowledge.rs:221-224`), so a
  user in two teams sharing one canonical id gets the **union** of both teams' pages, and a
  same-id collision picks one cross-team winner. cas-f177 is the client half.
- **Global/account scope: no representation.** `project_id` is `NOT NULL` server-side and the
  client fails closed without a canonical id on both directions, so global pages have no wire
  identity at all. This is a boundary by construction, not a bug.
- **Capability discovery: none.** `KnowledgeEmbedder::from_config` gates on
  `is_logged_in()` alone (`embeddings.rs:118-130`). Accepted direction is a `features[]` array
  on `/api/me`.

---

## Q3. What does pull return unscoped, and where does contamination enter?

### 3.1 The client never asks unscoped

**VERIFIED.** There is no unscoped pull path in shipped source. Both pull builders append
`project_id=` unconditionally and abort otherwise (`pull.rs:55-59`; team pull `:917`). The
comment at `pull.rs:29-34` records this as the explicit purpose of the builder. So
"what does pull return unscoped" is, for this client, **unreachable** — and per their §7.3-1
the server would `400` it anyway before running a query.

### 3.2 Contamination enters at exactly one place client-side, and it is the knowledge path

```
pull_knowledge_pages  (knowledge.rs:207)
  → build_scoped_pull_url(...)                     knowledge.rs:228   ← resolves project id
  → let (url, _project_id) = ...                   knowledge.rs:228   ← AND THROWS IT AWAY
  → for raw in incoming { apply_knowledge_record } knowledge.rs:257-273
                                                    ← zero entity_matches_project calls
```

`entity_matches_project` appears **nowhere** in `knowledge.rs`. `KnowledgePageRecord` even
carries `project_canonical_id` (`knowledge.rs:69`) and populates it on **push**
(`:87`) — it is simply never read on **pull**. Downstream, `apply_knowledge_record`
(`:288-303`) writes the page into `cas.db` *and* to disk via `commit_ingest`, keyed on
`rel_path`, so a foreign page with a colliding path **overwrites the local one unless the
local copy is locked**. Afterwards it is unattributable — `knowledge_pages` has no project
column.

So the split is unambiguous:

| Entity kind | Server scoping | Client re-check | Net |
|---|---|---|---|
| entries, tasks, rules, skills, specs, events, prompts, file_changes, commit_links | SERVER-ASSERTED filter + echo | **yes**, per row | defence in depth |
| team-pulled entries/tasks/rules/skills | SERVER-ASSERTED filter + echo | **yes**, per row | defence in depth |
| **knowledge pages** | SERVER-ASSERTED filter only | **none** | **single point of failure** |

**This is a client filter gap, not a server response defect** — on today's server. The
cloud team's §7.3-1 says `fetchVisibleKnowledgePages` does carry the `project_id` equality
filter, so no foreign page should arrive. The gap is that if that ever stops being true,
nothing on this side notices. cas-2cc5 owns closing it.

### 3.3 Three secondary contamination/loss vectors, all VERIFIED

1. **Case-variant canonical ids are silent data *loss*, not a leak.** `pull.rs:106` is a
   byte-exact `==`. Their production audit found two live case-collision pairs
   (`Accounting`/`accounting` = 11,110 rows; `Penguinz`/`penguinz` = 174). Fixing it on the
   server (lowercasing) would convert partial loss into a total blackout via this same line.
   Remedy is client-side: pin `[project] canonical_id`. Root cause is
   `resolve_canonical_id` falling through to the **parent folder name** without lowercasing
   (`cloud/config.rs:117-131`, SERVER-ASSERTED citation, matching our own config.rs).
2. **The knowledge pull watermark is client wall-clock.** `LAST_PULL_KEY` is set from
   `Utc::now()` after the loop (`knowledge.rs:275-277`), whereas the entity pull uses the
   **server-supplied** `pulled_at` (`pull.rs:506-508`). Knowledge is the lone holdout. Their
   5-minute lenient `since` exists precisely to absorb this; removing our clock from the
   protocol is a pure client change.
3. **The knowledge push watermark advances past server-refused pages.** `push_knowledge_pages`
   discards the `PushResponse` (`knowledge.rs:196`) and then sets the watermark to `now`
   (`:197-199`). A page the server refused under the locked guard is therefore never retried —
   it will not re-enter the `page.updated_at > since` window (`:172-176`) until it is edited
   again. The generic push path does handle this (`push.rs:324-345`); knowledge does not.

---

## Q4. What do 10,000 events / 4,343 file changes actually represent?

### 4.1 They are a snapshot, not a delta — and the "last 90 days" comments are wrong

**VERIFIED.** `cas cloud push` / `cas cloud sync` go through `execute_push`
(`cli/cloud.rs:1569`, invoked by `execute_sync` at `:2355`). It reads the **whole local
corpus** every run:

| Collection | Call | Bound |
|---|---|---|
| entries | `store.list()` `:1600` | **unbounded** |
| tasks | `task_store.list(None)` `:1607` | **unbounded** |
| rules / skills / specs / worktrees / task_deps | `list()` `:1614-1677` | **unbounded** |
| sessions | `list_sessions_since(now - 90d)` `:1625-1627` | genuinely 90 days |
| **events** | `list_recent(10000)` `:1639` | **newest 10,000, no time filter** |
| prompts | `list_recent(10000)` `:1645` | newest 10,000 |
| **file_changes** | `list_recent(10000)` `:1651` | newest 10,000 |
| commit_links | `list_recent(10000)` `:1657` | newest 10,000 |

The four `// Push … (last 90 days)` comments at `:1638, :1644, :1650, :1656` are **factually
wrong**. `list_recent` is `ORDER BY created_at DESC LIMIT ?1` with no predicate —
`crates/cas-store/src/event_store.rs:142-159` and
`crates/cas-store/src/file_change_store.rs:323-338` (prompts and commit_links identical).

**So the answer to the question as posed:**

- **10,000 events = the cap, saturated.** That number is not a measurement of the local
  corpus; it is `LIMIT 10000`. The true local event count is ≥ 10,000 and unknown from the
  plan. It is a **bounded window**, but the window is *newest-N*, not *newest-N-days*.
- **4,343 file changes = a real count**, below the cap, and **growing**. It will keep growing
  until it hits 10,000, at which point it silently becomes a cap too.
- **Entries and tasks are the genuinely unbounded ones** — the entire corpus is re-serialized
  and re-sent on every single push.

### 4.2 The bounded window is a correctness hazard, not just a bandwidth one

Because the window is newest-10,000 **with no watermark and no cursor**, once a table exceeds
10,000 rows the older rows **stop being pushed** — permanently, whether or not they ever
reached the server. There is no mechanism that notices. Combined with a fresh install (or the
GH #158 situation below), rows older than the newest 10,000 can never be uploaded at all.

Conversely, everything inside the window is re-sent every run: at `BATCH_SIZE = 50`
(`cli/cloud.rs:1744`), a saturated corpus is ~200 HTTP round trips per sync for events alone,
and the batching is *column-parallel* (chunk `i` of every type in one payload, `:1807-1841`),
so the request count is driven by the largest single collection.

### 4.3 …and `sync_queue` is not what drives any of it

This is the structural finding behind **GH #158** ("cloud: sync_queue is write-only — push
ignores it entirely and rows accumulate forever (never drained since April)", OPEN).

**VERIFIED:** there are two personal push implementations and the CLI uses the wrong one.

- **Queue-driven:** `CloudSyncer::push_with_sessions` (`push.rs:19`) reads
  `pending_by_type(batch_size, max_retries)` (`:27-29`) and, on success, calls
  `queue.mark_synced(id)` which **deletes the row**
  (`cloud/sync_queue/maintenance.rs:9-13`). Its only caller is `sync_with_sessions`
  (`pull.rs:788`), whose only caller is the **daemon** (`mcp/daemon.rs:1274`).
- **Snapshot-driven:** `execute_push` (`cli/cloud.rs:1569`) — never touches `SyncQueue`
  except to run the rehome guard (`:1779-1794`). This is what `cas cloud sync` calls
  (`:2355`).

So: on a machine where the daemon's cloud syncer never runs, `sync_queue` is append-only. The
measured live state (248 pending / **0 failed** / entry:14, task:234) is consistent with
exactly that and inconsistent with a *failing* drain — a failing drain would increment
`retry_count` via `mark_failed` (`maintenance.rs:16-`) and those rows would appear under
`failed`. **Zero failed means the drain never executed on these rows at all.**

**UNKNOWN (runtime, not code):** *why* the daemon path never drained on this machine.
Candidates readable from code: `cloud_syncer` is `None` unless
`CloudConfig::load_from_cas_dir(cas_root)` reports logged-in at daemon start
(`mcp/daemon.rs:1558-1572`, `:150`), and the drain only fires past an idle threshold
(`:510`). Distinguishing these needs runtime evidence, which this spike is not authorized to
gather.

**Knowledge pages are not in the queue by design, not by omission.** `EntityType::KnowledgePage`
exists (`sync_queue/types.rs:24, :42, :60`) and `stats.rs:97` groups it, but nothing ever
enqueues it: `team_push.rs:380` and `:404` carry explicit empty match arms with the comment
*"Pages ship via `push_knowledge_pages` (it needs the body from disk), so nothing enqueues
them today. Arm kept explicit so a future queue producer is a compile error."* The measured
"ZERO knowledge rows in sync_queue" is therefore the designed state — knowledge runs its own
watermark protocol (`LAST_PUSH_KEY`/`LAST_PULL_KEY`, `knowledge.rs:34-37`) entirely outside
the queue.

**One more consequence the cloud team already flagged (their §11.5):** because the snapshot
path re-sends the whole corpus and has **no delete handling at all**, a locally-deleted entry
is not merely orphaned server-side — it is **re-asserted on every sync from every machine
that still holds it**. That is why any server-side cleanup (including the five fixture
strings) must follow the local purge, never precede it.

---

## Summary: what the redesign is actually working against

1. **Scoping is enforced twice for every entity type except knowledge pages** — where it is
   enforced once, on a server we cannot test. (cas-2cc5)
2. **The queue exists, works, deletes on success, and is bypassed by the command users run.**
   The redesign's "queue-driven push" is not new machinery — it is routing `cas cloud sync`
   to the implementation that already exists. (cas-cb6e, GH #158)
3. **"10,000 events" is a cap, not a corpus.** Any plan output should distinguish
   `count == limit` from `count < limit`; today it cannot. Entries/tasks are the truly
   unbounded ones.
4. **The server shipped more than we assumed** — knowledge sync including the locked guard,
   and a live, contract-pinned embeddings endpoint. The remaining server-side gaps are
   tombstones, `team_id`-on-knowledge-pull (our half), account scope, and capability
   discovery — all four already have named owners.
5. **`pull.rs:106` is the most dangerous line in the sync client**: a byte-exact comparison
   whose failure mode is a total, silent, warning-only blackout. Both sides now know it; it
   should be stated as a protocol invariant, not left as an implementation detail.

## Files read

| File | Why |
|---|---|
| `cas-cli/src/cloud/syncer/pull.rs` | pull URL building, per-row project filter, team pull, sync orchestration |
| `cas-cli/src/cloud/syncer/push.rs` | queue-driven personal push, sub-batching, skipped handling, deletes |
| `cas-cli/src/cloud/syncer/knowledge.rs` | knowledge push/pull, wire record, watermarks, the filter gap |
| `cas-cli/src/cloud/syncer/team_push.rs` | team push envelope, `git_remote`, KnowledgePage no-op arms |
| `cas-cli/src/cloud/syncer/mod.rs` | `is_available`, `CloudSyncerConfig` defaults |
| `cas-cli/src/cloud/embeddings.rs` | model/dims constants, capability gate, response parsing, batching |
| `cas-cli/src/cloud/sync_queue/{queue_ops,maintenance,schema,stats,types}.rs` | enqueue/pending/mark_synced semantics, entity types |
| `cas-cli/src/cli/cloud.rs` | `execute_push` snapshot plan, `execute_sync`, `execute_pull`, `sync_project_knowledge` |
| `cas-cli/src/mcp/daemon.rs`, `cas-cli/src/mcp/server/runtime.rs` | the only callers of the queue-driven path |
| `crates/cas-store/src/{event_store,file_change_store}.rs` | `list_recent` has no time filter |
| `docs/requests/completed/RESPONSE-cloud-knowledge-sync-and-embeddings.md` | every SERVER-ASSERTED claim above |
