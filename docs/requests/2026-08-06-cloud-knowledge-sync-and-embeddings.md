---
from: CAS CLI team
to: CAS Cloud team
date: 2026-08-06
priority: P1
pinned_client: cas-cli @ 845ace3f (impl) / fdef6ce8 (style), merged to the knowledge epic as 9424b300
---

# Feature Request: server-side knowledge-page sync + an embeddings endpoint

## What already shipped, and what is missing

The **client half** of cloud knowledge is merged and released-ready. A CAS install that is
logged in will, on every `cas cloud sync`:

1. push distilled knowledge pages to `POST /api/sync/push` under a new entity key
   `knowledge_pages`,
2. pull teammates' pages from `GET /api/sync/pull?types=knowledge_pages`,
3. `POST /api/embeddings` for every page still awaiting a vector, and cache the vectors
   locally.

**None of those three server behaviours exist yet.** Today the feature is exercised only
against `wiremock` fixtures in the client's own tests. Those fixtures are, in the literal
sense, the current contract — this document transcribes them plus the surrounding client
code so the cloud team can build against a pinned, real shape rather than a sketch.

Everything below is traceable to a struct field, a serde attribute or a test in the two
client modules:

- `cas-cli/src/cloud/syncer/knowledge.rs` — push/pull of pages
- `cas-cli/src/cloud/embeddings.rs` — the embedding client and the local vector cache
- `cas-cli/src/hybrid_search/semantic.rs` — the retrieval side that consumes the vectors

No field named here is aspirational. If a name is not in this document, the client does not
send it and will not read it.

## The boundary this must not cross

From `cas-cli/docs/ARCHITECTURE.md`, "The local/cloud boundary for project knowledge":

| Concern | Owner |
|---|---|
| Pages, bodies, provenance, the `locked` bit | **Local** — SQLite + markdown on disk. Fully functional with no account, no network, no cloud build. |
| Embedding vectors | **Cloud computes, local caches.** |
| Team distribution of pages | **Cloud transports.** |

**Local is the source of truth. The cloud is an optional enhancement, never a dependency.**
The server is a transport and a compute service for this data; it never becomes
authoritative. A machine that never talks to the cloud has a fully working knowledge base,
and no cloud response may be required for local retrieval to function. Please do not design
server behaviour that assumes it can be the arbiter of page content — the client will not
honour it (see "Conflict rules", below).

---

## 1. `/api/sync` — knowledge pages

### 1.1 Push

`POST {endpoint}/api/sync/push`, `Authorization: Bearer <token>`,
`Content-Type: application/json`, `Content-Encoding: gzip`. The client retries up to 3
times with backoff and accepts `200` or `201`
(`cas-cli/src/cloud/syncer/push.rs:451-548`).

Knowledge pages reuse the **existing** push envelope built by `push_sub_batch`
(`push.rs:451-483`) — there is no new route. The gzipped JSON body is:

- `knowledge_pages`: array of page records (the entity key; `KNOWLEDGE_ENTITY`,
  `knowledge.rs:39`)
- `team_id`: present only when a team is configured (`push.rs:465-467`)
- `project_canonical_id`: required; the client refuses to sync outside a CAS project
  (`push.rs:469-474`)
- `client_version`, `client_build` (`push.rs:549-558`)

Response: the client parses `{"skipped": {"<entity_type>": <count>}}` and treats an empty
or unparseable body as `{}` for backward compatibility (`push.rs:498-511`,
`syncer/mod.rs:412-417`). A `skipped` count for `knowledge_pages` is understood as
"the server rejected these rows as a cross-project conflict".

### 1.2 The page record

One element of the `knowledge_pages` array. Every name is the serde name on
`KnowledgePageRecord` (`knowledge.rs:46-70`):

| Field | Type | Notes |
|---|---|---|
| `id` | string | Client-generated page id, e.g. `cas-kn001`. Stable across machines. |
| `page_type` | string | e.g. `architecture`. |
| `title` | string | |
| `rel_path` | string | Canonical on-disk path of the markdown body. **The receiver must not recompute it** — see 1.4. |
| `snippet` | string | `#[serde(default)]`; empty string when absent. |
| `body` | string | Full markdown, inline. Pages are kilobytes and the payload is gzipped, so there is no separate blob fetch. `#[serde(default)]`. |
| `locked` | bool | User-sovereignty bit. `#[serde(default)]` → false. See §3. |
| `sources` | array of string | Provenance source ids. `#[serde(default)]`. |
| `created_at` | RFC 3339 timestamp (UTC) | |
| `updated_at` | RFC 3339 timestamp (UTC) | The incrementality watermark; see 1.3. |
| `share` | `"private"` \| `"team"` \| omitted | `Option<ShareScope>`, `skip_serializing_if = "Option::is_none"`. Lowercase on the wire (`crates/cas-types/src/scope.rs:166-173`). Absent on older payloads and **must be read as `private`** (`knowledge.rs:64-67`). |
| `project_canonical_id` | string \| omitted | Same value as the envelope's, per record. |

`share` is set by `knowledge_share_scope` (`knowledge.rs:135-141`): `team` when a team is
configured, `private` otherwise. There is no per-page override today — `KnowledgePage` has
no `share` column — so the server should treat `share` as a *distribution* instruction for
this push, not as durable per-page metadata the client will later reconcile.

### 1.3 Incrementality

The client keeps two high-water marks in its local sync-queue metadata
(`knowledge.rs:35-37`): `last_knowledge_push_at` and `last_knowledge_pull_at`.

- **Push** sends only pages with `updated_at > last_knowledge_push_at`
  (`knowledge.rs:172-176`), then stamps the mark with `Utc::now()` *after* a successful
  push (`knowledge.rs:197-199`).
- **Pull** sends `since=<last_knowledge_pull_at>` and stamps the mark with `Utc::now()`
  after the response is processed (`knowledge.rs:222-224`, `:278-280`).

Both marks are wall-clock stamps taken on the **client**. The server therefore must not
assume the two clocks agree; `since` should be applied leniently (a page returned twice is
harmless — see §3 — a page skipped once is lost until its next local edit). Clock skew is
an open question, §5.

Fixture: `pushes_pages_and_advances_the_high_water_mark` (`knowledge.rs:412-442`) asserts
the seeded page is pushed once and the unchanged page is **not** re-pushed.

### 1.4 Pull

`GET {endpoint}/api/sync/pull?types=knowledge_pages[&since=<rfc3339>][&project_id=<id>]`,
`Authorization: Bearer <token>` (`knowledge.rs:221-237`). `project_id` is
percent-escaped on `/` only (`knowledge.rs:225-227`) — the canonical id is
`owner/repo`-shaped.

Response body, `200`:

- Top level object with the key `knowledge_pages` holding an array of page records in the
  same shape as §1.2 (`knowledge.rs:254-258`).
- **Any other envelope silently yields zero pages.** The client reads exactly
  `body["knowledge_pages"]` as an array and defaults to empty otherwise. This is the single
  easiest way to ship a server that "works" and delivers nothing, so it is worth a
  server-side test.

Error handling the client relies on (`knowledge.rs:243-251`): a non-2xx status becomes a
hard error carrying the status and body text; a transport failure becomes a network error.
Both abort the knowledge pull for that run (the rest of `cas cloud sync` continues — the
knowledge tail is non-fatal). A malformed individual record does **not** abort the pull: it
is recorded as a per-page error and the remaining records are still applied
(`knowledge.rs:260-276`).

Fixtures: `pulls_pages_and_preserves_a_locked_local_page` (`knowledge.rs:444-501`) and
`a_pulled_page_round_trips_its_locked_bit` (`knowledge.rs:503-543`).

### 1.5 What arriving pages become locally

`KnowledgePageRecord::into_page_write` (`knowledge.rs:96-112`):

- `rel_path` is taken from the sender verbatim, deliberately: recomputing the slug would
  let two CAS versions with different slug rules fork one page into two paths across
  machines (`knowledge.rs:98-101`).
- `locked` is carried through (§3).
- **`pending_embedding` is forced to `true`** (`knowledge.rs:107`). A vector computed on a
  teammate's machine lives in *that machine's* local LMDB cache and is never transmitted.
  Every receiving machine embeds pulled pages itself.

The consequence for capacity planning: **a team of N members generates roughly N embedding
calls per page**, not one. There is no server-side vector fan-out today and the client does
not want one — vectors are only comparable within one `(provider, model, dims)` space and
the client owns that identity locally (§2.3). If the cloud wants to dedupe that work, it is
a separate design conversation, not something the current client can consume.

---

## 2. `/api/embeddings`

### 2.1 Request

`POST {endpoint}/api/embeddings` (`embeddings.rs:174`), with
`Authorization: Bearer <token>` and `Content-Type: application/json`. Client timeout is 30s
(`embeddings.rs:128`). Body (`embeddings.rs:175-178`):

```json
{ "model": "cas-embed-v1", "input": ["<text>", "<text>"] }
```

- `model` defaults to `cas-embed-v1` (`DEFAULT_EMBEDDING_MODEL`, `embeddings.rs:53`) unless
  a caller overrides it with `with_model`.
- `input` is an array of strings. Indexing sends `title\n\n snippet\n\n body` per page
  (`page_embedding_text`, `embeddings.rs:463-465`); retrieval sends a single raw query
  string (`semantic.rs:76`). Both go to the same endpoint and the same model — they must
  land in the same vector space or ranking is meaningless.
- Batch size is capped at 32 per invocation by default (`DEFAULT_EMBED_BATCH`,
  `embeddings.rs:62`) so a first run on a large repo cannot become an unbounded burst.

### 2.2 Response

Either shape is accepted (`parse_embedding_response`, `embeddings.rs:220-236`):

```json
{ "embeddings": [[0.1, 0.2], [0.3, 0.4]] }
```

```json
{ "data": [{ "embedding": [0.1, 0.2] }] }
```

Both are supported so the cloud can settle on either without forcing a client re-release.
A body with neither key is a hard client error
(`"Embedding response had neither ``embeddings`` nor ``data[].embedding``"`,
`embeddings.rs:201-205`).

**Vector count must equal input count.** `vectors.len() != texts.len()` is a hard error
(`embeddings.rs:207-213`) — the client zips vectors to pages positionally
(`embeddings.rs:505`), so a short or reordered response would attach the wrong vector to
the wrong page. Order is significant and must match `input`.

### 2.3 Model identity — `{provider, model, dims}`

The local cache is tagged with an `EmbeddingMeta { provider, model, dims }`
(`embeddings.rs:64-84`) persisted as `embedding_meta.json` beside the LMDB environment in
`.cas/index/knowledge-vectors/`. `provider` is the constant `cas-cloud`
(`embeddings.rs:50`); `model` and `dims` come from the client config, defaulting to
`cas-embed-v1` / `1024` (`embeddings.rs:53-58`).

On any mismatch between the persisted triple and the requested one, the client **destroys
the entire cache and re-marks every page `pending_embedding`**
(`KnowledgeVectorCache::open`, `embeddings.rs:322-365`; `mark_all_pending_embedding`,
`embeddings.rs:484-488`). Vectors from two models are not comparable and mixing them
corrupts ranking silently, so this is deliberate and not tunable.

**Hard requirement:** the server must not silently change the model, dimensionality or
normalization behind a stable `model` string. A model swap needs a new `model` identifier,
or every client cache in the field becomes a mix of incomparable vectors with no signal
that anything changed. Conversely, a *declared* model change is cheap: it costs one full
re-embed per machine and is fully automatic. Please prefer visible renames over invisible
upgrades.

Fixtures: `model_change_wipes_the_cache_and_flags_reindex` (`embeddings.rs:616-634`),
`reopening_with_the_same_meta_preserves_vectors` (`embeddings.rs:636-648`).

### 2.4 Error semantics the client depends on

These are requirements, not preferences — each maps to a client behaviour that will
misfire if the server does something else.

**(a) Never return a zero vector.** `is_zero_vector` (`embeddings.rs:254-256`) treats an
empty vector or an all-zero vector as unusable. `KnowledgeVectorCache::put`
(`embeddings.rs:393-398`) *refuses* to cache one, and `embed_pending_pages`
(`embeddings.rs:506-509`) counts it as `rejected_zero` and leaves the page
`pending_embedding = 1` so the next run retries. A zero vector has cosine similarity 0
against every query, so caching one is strictly worse than caching nothing: the page would
look embedded and be permanently unretrievable.

  *Requirement:* a provider failure must be an **error status**, never a soft-failed
  all-zero row. If the upstream model errors, return non-2xx. The client is built to retry;
  it is not built to distinguish a legitimate zero from a failure.

  Fixtures: `zero_vectors_are_never_cached` (`embeddings.rs:597-604`),
  `a_zero_vector_from_the_provider_leaves_the_page_pending` (`embeddings.rs:735-774`) —
  the latter asserts `embedded == 0`, `rejected_zero == 1`, cache count 0, still pending 1.

**(b) 5xx must degrade, and does.** On the retrieval path a provider error is swallowed:
`SemanticChannel::search` returns an empty result list rather than an error when the query
embedding fails (`semantic.rs:72-95`), so a flaky endpoint degrades retrieval instead of
failing the whole search. The hybrid scorer then redistributes that channel's weight to the
live channels. Fixture: the wiremock `ResponseTemplate::new(500)` test at
`semantic.rs:200-229` — *"a 500 from the embedding provider must not fail the search"*.

  Practical consequence: **the client will not surface your outage loudly.** Degradation is
  silent by design, so server-side error rates are the only signal that the semantic channel
  is down for a fleet. Please instrument accordingly.

**(c) Dimension must be stable within a model.** A vector whose length differs from the
cache's `dims` is rejected (`embeddings.rs:399-405`, counted as `rejected_dims` at
`embeddings.rs:510-513`) and the page stays pending — it will retry forever against a
server that keeps returning the wrong width. Fixture: `dimension_mismatch_is_rejected`
(`embeddings.rs:606-613`).

**(d) Auth absence is a client-side state, not a server concern.** `KnowledgeEmbedder::from_config`
returns `None` when the user is not logged in (`embeddings.rs:118-130`), and the sync entry
points return empty without touching the network (`knowledge.rs:148-151`, `:212-214`). A
logged-out install makes zero requests and creates no vector storage on disk, so the server
will never see traffic from one. Fixture:
`logged_out_push_and_pull_make_no_network_calls` (`knowledge.rs:399-410`), which points the
client at an unroutable endpoint and asserts both paths return empty.

---

## 3. Conflict rules the client already assumes

### 3.1 A locked page must never be overwritten — including server-side

`locked` marks a page a human took ownership of. Locally the guarantee is enforced in SQL:
`commit_ingest`'s upsert carries `ON CONFLICT ... WHERE knowledge_pages.locked = 0`, so
neither distillation nor an incoming teammate copy can overwrite a locked page
(`cas-cli/docs/ARCHITECTURE.md:66`, `:88`). The pull path applies every incoming record
through exactly that function (`apply_knowledge_record`, `knowledge.rs:291-306`) and reports
a refused write as `locked_preserved`, not an error (`knowledge.rs:117-125`, `:271-275`).

**Hard requirement:** the server must enforce the same guard on push —
*a pushed page must not overwrite a stored page whose stored `locked` is true, unless the
push comes from the account that locked it.*

Why this is not optional: the client's guard protects the *local* copy only. If the server
lets teammate B's push clobber the server-side row for a page teammate A locked, then A's
own next pull re-delivers B's body under A's page id. A's local guard still refuses the
write — so A is safe — but the canonical shared copy is now B's, every *other* teammate
receives B's version, and the lock A took has silently stopped meaning anything for the
team. The bit is transmitted precisely so it can be honoured on both ends
(`knowledge.rs:56-59`, `:82`, `:103`).

Two client fixtures pin the local half of this and are the behaviour the server must mirror:

- `pulls_pages_and_preserves_a_locked_local_page` (`knowledge.rs:444-501`) — a remote page
  whose body is `# REMOTE OVERWRITE` collides with a locally locked page; the test asserts
  the local body still contains `Zig linker`, that `REMOTE OVERWRITE` is absent, and that
  the page counts as `locked_preserved` rather than `applied`.
- `a_pulled_page_round_trips_its_locked_bit` (`knowledge.rs:503-543`) — a page locked
  upstream must arrive locked, *"or the next local distillation pass would silently
  overwrite a human-owned page"*.

### 3.2 Everything else is last-writer-wins on `updated_at`

There is no vector clock, no revision counter and no merge. A page is identified by `id`
(with `rel_path` as the on-disk identity), and the newest `updated_at` wins. That is
acceptable because pages are regenerable distillations, not user-authored source — except
when `locked` is set, which is exactly the case §3.1 covers.

### 3.3 Redelivery is safe; loss is not

Applying the same record twice is idempotent (same id, same `rel_path`, same body). A
record that is skipped, however, is invisible until the source page is edited again. When
in doubt about a `since` boundary, **return the page**.

### 3.4 Per-record application, not per-batch

The client commits pulled pages **one at a time** on purpose: `commit_ingest` aborts the
whole batch on an id/`rel_path` collision, and one teammate's odd page must not discard
every other page in the same pull (`knowledge.rs:285-290`). The server may batch freely in
the response; the client will not let a single bad record poison the run.

---

## 4. Suggested acceptance for the server work

Framed as things the existing client tests would pass against a live server:

1. A push of one page, then an unchanged re-push, results in one stored row and the second
   push carrying zero records.
2. A pull with no `since` returns every page visible to the caller under the
   `knowledge_pages` key; a pull with `since` returns only pages newer than it, and
   over-returning is preferred to under-returning.
3. `share: "team"` pages reach teammates in the same team; `share: "private"` pages reach
   only the pushing account.
4. A push from account B against a page stored with `locked: true` and owned by account A is
   rejected or skipped (reported via the `skipped` counter), and A's body remains canonical.
5. `POST /api/embeddings` with 2 inputs returns exactly 2 vectors, in order, of the declared
   dimension, non-zero.
6. Upstream provider failure returns a non-2xx status — never a 200 with zero vectors.

---

## 5. Open questions for the cloud team

1. **Auth scoping.** Is a token scoped to an account, an org, or a team? Knowledge pages are
   pushed with both `team_id` (when configured) and `project_canonical_id`. Which one gates
   read visibility on pull — and what does a user who belongs to two teams sharing one
   project canonical id see?
2. **`share` durability.** The client sends `share` per record but has no per-page `share`
   column locally; the value is derived from whether a team is configured at push time. If a
   user later unlinks the project from the team, should previously pushed `team` pages be
   retracted, or is `share` purely a point-in-time distribution instruction? The client
   cannot re-push a retraction today.
3. **Clock skew and `since`.** Both watermarks are client wall-clock stamps. Should the
   server instead return an opaque cursor the client echoes back? That would be a client
   change, but it is the durable fix and worth deciding before there are many installs.
4. **Rate limits.** Recall that every teammate embeds every pulled page independently
   (§1.5), so embedding volume scales with `pages × team size`, in bursts of up to 32 inputs
   per call. What limits should the client expect, and what is the retry signal — `429` with
   `Retry-After`? The client currently retries push with backoff (3 attempts) but does
   **not** retry embeddings within a run; it leaves pages pending for the next run.
5. **Embedding model and dimensionality.** `cas-embed-v1` / 1024 dims are client-side
   placeholders (`embeddings.rs:53-58`), not a decision. What model will actually back the
   endpoint, what dimensionality does it return, and are vectors L2-normalized? The client
   scores with plain cosine similarity (`embeddings.rs:260-276`), so unnormalized vectors
   are fine, but the answer determines the honest default.
6. **Response shape.** The client accepts both the flat and the OpenAI-compatible shapes;
   please pick one and tell us, so the other can eventually be retired.
7. **Tenancy and body storage.** Page bodies are full markdown from private repositories
   travelling inline. Where are they stored, for how long, and are they used for anything
   other than transport and embedding? This is the question a customer will ask first, and
   the client-side answer today is "local is the source of truth, the cloud only carries
   it" — the server needs to be able to say the same.
8. **Deletion.** There is no tombstone path for knowledge pages on the wire today. When a
   page is deleted locally, teammates keep their copy. Should `/api/sync` carry knowledge
   tombstones, and if so under what key?

---

## Reference: files to read

| File | What it defines |
|---|---|
| `cas-cli/src/cloud/syncer/knowledge.rs` | Page record, push/pull, watermarks, locked handling. Tests from line 322 are the wire fixtures. |
| `cas-cli/src/cloud/syncer/push.rs:451-558` | The shared push envelope and its response shape. |
| `cas-cli/src/cloud/embeddings.rs` | Embedding request/response, `EmbeddingMeta`, the vector cache and its invariants. Tests from line 526. |
| `cas-cli/src/hybrid_search/semantic.rs` | The retrieval side; the 500-degrades-gracefully fixture. |
| `cas-cli/docs/ARCHITECTURE.md` (local/cloud boundary) | The boundary statement and its six invariants. |
| `crates/cas-types/src/scope.rs:153-193` | `ShareScope` — the `private` \| `team` vocabulary. |

Pinned at client commits `845ace3f` and `fdef6ce8` (epic merge `9424b300`). If those files
have moved by the time you read this, the commits still describe the contract this document
transcribes.
