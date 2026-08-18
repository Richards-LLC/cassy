---
from: Cassy CLI team
to: Cassy Cloud team
date: 2026-08-06
amended: 2026-08-07
priority: P1
pinned_client: cas-cli @ 845ace3f (impl) / fdef6ce8 (style), merged to the knowledge epic as 9424b300
amendment_client: cas-cli @ 34d5279b (epic tip); migration reality pinned at 7885ed2f on the cas-b129 epic branch
---

> **Disposition (2026-08-07, cas-ab75):** ANSWERED — outbound request to the Petra Stella Cloud team (not a cas-cli defect, so no GitHub issue in this repo). The cloud team replied in full: see `RESPONSE-cloud-knowledge-sync-and-embeddings.md` in this directory (status: COMPLETE — all eight original questions and all eight amendment questions answered; their task cas-369a). Archived together with its response.

# Feature Request: server-side knowledge-page sync + an embeddings endpoint

> ## Amendment — 2026-08-07
>
> **Read this box before §1.** The body below is unchanged except where a subsection is
> marked ⟨A⟩; every original claim in it still holds. What changed is the *world around it*:
> knowledge pages are becoming Cassy's **memory system**, not only a distillation of a
> repository, and that promotes three of the original open questions into requirements.
>
> | # | What changed | Where |
> |---|---|---|
> | 1 | Migrated legacy memories arrive as pages carrying ~27 reserved `cas_legacy_*` **frontmatter** keys. This adds **no new wire fields** — they ride inside the existing `body` string — but it does impose a *do-not-reserialize* constraint. | new §6 |
> | 2 | `cas_legacy_team_id` is **inert provenance**. A page carrying it is **not** team-scoped. The server must never derive `share`, an ACL or a visibility default from it. | new §6.3 |
> | 3 | Server-side project scoping is no longer a nicety. **The knowledge pull path performs no client-side project filter at all** — unlike every other entity type — so a foreign row the server returns is written to disk with zero detection. Four documented contamination incidents are cited. | new §7 |
> | 4 | §2.3's claim that `model`/`dims` "come from the client config" is **wrong**; they are compile-time constants. A model rename needs a **client release**, not a config change. | ⟨A⟩ §2.3 |
> | 5 | "Capability-gated" (cas-1ac6) means a **local token check**, not feature discovery. There is no capability handshake anywhere. A logged-in client retries a missing `/api/embeddings` silently and forever. | ⟨A⟩ §2.5 |
> | 6 | Entities and pages will both carry the same memories during the migration window, pushed by different code paths with no correlation field. | new §8 |
> | 7 | Incrementality: entity pull **already** echoes a server-supplied `pulled_at`; knowledge pull is the outlier still using a client clock. GH #158 makes the queue's future explicitly undecided. | ⟨A⟩ §1.3, new §9 |
> | 8 | An explicit **out-of-scope** list, which this document previously lacked. | new §10 |
>
> Open questions from the original §5 that are now **answered** are marked there. New ones are
> appended as §11. Nothing below has been deleted.

## What already shipped, and what is missing

The **client half** of cloud knowledge is merged and released-ready. A Cassy install that is
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
- `project_canonical_id`: required; the client refuses to sync outside a Cassy project
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

> **⟨A⟩ 2026-08-07 — knowledge pull is the *outlier*, and the fix is already shipped elsewhere.**
> The statement above is true for knowledge pages and *only* for knowledge pages. The
> **entity** pull has used a **server-supplied** position for some time: it reads
> `body.pulled_at` off the pull response and stores it verbatim as the next `since`
> (`pull.rs:505-508`), and the team pull does the same per `(team, project)`
> (`pull.rs:1061-1062`). `pulled_at` is a declared field of the server's `PullResponse`
> (`syncer/mod.rs:330`).
>
> So original open question §5.3 — "should the server return an opaque cursor the client
> echoes back?" — is **already answered yes, and implemented twice**. Knowledge pull simply
> has not adopted it. Please keep returning `pulled_at`; the client-side change to make
> knowledge pages read it is small and well-precedented. Until it lands, keep `since=`
> lenient exactly as §3.3 asks.
>
> Two honest caveats: the client also parses `last_pull_at` as RFC 3339 for staleness and
> purge guards (`cloud.rs:3449`, `:3620-3629`), so a *fully* opaque cursor would need those
> two call sites revisited first; and knowledge pull does not read `pulled_at` at all today.

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
  let two Cassy versions with different slug rules fork one page into two paths across
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

> **⟨A⟩ 2026-08-07 — correction: `model` and `dims` do NOT come from the client config.**
> This is the one factual error in the original document and it understates a constraint.
> `KnowledgeEmbedder::from_config` hardcodes both constants (`embeddings.rs:126-127`); the
> only override is the programmatic `with_model(model, dims)` builder
> (`embeddings.rs:143-147`), which only tests use. There is **no config key, no env var and
> no CLI flag** for the embedding model or dimensionality anywhere in the tree.
>
> The model string is therefore a **compile-time constant baked into each released binary**.
> That sharpens "prefer visible renames over invisible upgrades" below into a hard
> operational fact: **a server-side model rename cannot be absorbed by configuration in the
> field — it requires a client release, and old binaries in the field will keep asking for
> the old string.** Please treat `cas-embed-v1` as stable across the whole supported
> client-version window, and see new question §11.2.

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

---

# Amendment sections (2026-08-07)

Everything from here down was added by the 2026-08-07 amendment. The client reality it
describes is pinned at epic tip `34d5279b`, plus the **cas-b129 migration branch**
(`epic/memory-knowledge-migration-carry-the-legacy-memory-cas-b129`, spec added by
`7885ed2f`). Where a fact lives only on that branch and has not merged, it says so — those
are commitments, not shipped behaviour, and you should not build against them until we
confirm the merge.

## 6. Migrated legacy memories arrive as pages

Cassy is collapsing its legacy `entries` memory store into knowledge pages (epic cas-b129).
The migration's mapping spec is **normative**: "M3 implements this document literally; where
it says MUST, a migration that does otherwise is wrong" (`cas-b129-mapping-spec.md:3-4`).

### 6.1 The good news: no new wire fields

Legacy state rides as reserved `cas_legacy_*` **YAML frontmatter keys inside the page body**,
because `knowledge_pages` has no columns for any of it (spec:30-46, "Rule C1"). Since `body`
is already a wire field (§1.2), **the page record shape in §1.2 does not change.** There is
nothing new for you to accept, store or index in order to receive migrated memories.

The full set is 27 keys (spec:63-93, "Rule C2 — the complete set; M3 emits no others"),
implemented at `cas-cli/src/memory_migration/frontmatter.rs:26-52`. Examples:
`cas_legacy_id`, `cas_legacy_db`, `cas_legacy_scope`, `cas_legacy_type`,
`cas_legacy_memory_tier`, `cas_legacy_importance`, `cas_legacy_created`,
`cas_legacy_updated_at`, `cas_legacy_tags`, `cas_legacy_team_id`.

Three details that bite:

- **Absence means "default", not "null".** Rule C3 (spec:95-102): a key is emitted only when
  its column is non-NULL **and differs from the schema default**. Only `cas_legacy_id`,
  `cas_legacy_db` and `cas_legacy_scope` are unconditional
  (`frontmatter.rs:145-147`). Do not infer that a missing key means the source had no value.
- **`cas_legacy_created` has no `_at`; `cas_legacy_updated_at` does.** The asymmetry mirrors
  the legacy column names (spec:82-83, `frontmatter.rs:41-42`). If you write
  `cas_legacy_created_at` anywhere, it is wrong.
- **`cas_legacy_scope` and `cas_legacy_db` can legitimately disagree.** Scope is *derived*,
  never copied — the legacy `scope` column is known-false (all 450 global rows claim
  `scope='project'`), so it is refused and re-derived from the id prefix first, the source
  database file second (spec:255, `frontmatter.rs:76-84`). Do not assume the two fields are
  equal, and do not "repair" one from the other.

### 6.2 Hard requirement: do not re-serialize frontmatter

Cassy's frontmatter reader is **hand-rolled, not a YAML engine** (`merge.rs:117-120`; Rule C4,
spec:104-109). It requires flat scalars, permits exactly one list block
(`cas_legacy_tags`, emitted as flat `- item` lines), and supports no nested maps, no
multi-line strings and no anchors.

**Requirement:** if the server stores and returns page bodies, it must return the byte
sequence it received. Round-tripping a body through a real YAML parser/emitter — which will
happily re-quote scalars, reorder keys, fold long lines or emit an anchor — produces a body
the client's reader mis-parses. Because a malformed block "simply yields defaults"
(`merge.rs:116-119`), the failure is **silent**: `locked` reads false, and the next
distillation pass overwrites a page a human owned. Treat `body` as an opaque blob.

### 6.3 Hard requirement: `cas_legacy_team_id` is INERT

Two normative rules (spec:347-359):

> - **Rule S1.** M3 MUST NOT synthesize a `share` value from `team_id`. The destination has
>   no sharing enforcement at all, so any synthesized value would be an unenforced
>   assertion — worse than absence.
> - **Rule S2.** Because the destination cannot enforce team scoping, a page carrying
>   `cas_legacy_team_id` is *not* thereby team-scoped.

626 rows carry a `team_id` into a destination with **no team enforcement whatsoever** —
`knowledge_pages` has no `team_id` and no `share` column
(`knowledge_store.rs:57-96`). The client refuses to invent semantics it cannot enforce, and
asks the server to do the same.

**Requirement:** treat `cas_legacy_team_id` as read-only origin metadata. Do **not** derive
from it: a `share` value, team membership, an ACL, a distribution list, or a visibility
default. These are legacy ids from a system that never enforced them; they carry no
guarantee of naming a currently-valid team.

The field that *does* carry distribution intent is the existing wire field `share`
(§1.2), set by `knowledge_share_scope` (`knowledge.rs:135-141`). The two must never be
folded together.

*Status note, stated precisely because it matters:* the mapping spec records this as
**ESCALATION E3** — "confirm that carrying it as inert provenance is acceptable for now;
*Recommendation: accept; note as a known gap*" (spec:400-402). Rules S1/S2 are normative and
binding on the client today; E3 is the open escalation about whether the resulting gap is
tolerable. Either way, the ask on the server is identical: **do nothing with the field.**

### 6.4 Byte identity, precisely scoped

Some migrated pages are `carry-verbatim`: the body is **byte-identical to the legacy content
beneath the frontmatter block**, and the page is created **locked** (spec:15, :196, :299).
Rule L1 is emphatic — a `carry-verbatim` page that ends up unlocked "is a migration failure
and M3 MUST fail the run, not warn" (spec:338-342).

Scope of the guarantee, stated exactly so you do not over-apply it:

- **What bytes:** the legacy content bytes below the frontmatter. The *file* is not
  byte-identical — a `cas_legacy_*` block is prepended. Anything that normalizes line
  endings, trims trailing whitespace or re-wraps text breaks it.
- **Which pages:** only `carry-verbatim` rows — pinned memories (`memory_tier='in_context'`)
  and user preferences (`type='preference'`), 21 rows on the reference corpus (spec:160-161).
  Ordinary migrated pages are created **unlocked** precisely so distillation may rewrite them
  (Rule L2, spec:343-345).

This is the strongest possible argument for the locked-overwrite requirement already stated
in §3.1, and it is why that requirement is not negotiable: these pages are **human-authored
intent that must never be re-worded**. §3.1 asks the server not to let a teammate's push
clobber them; §6.2 asks it not to let a *storage round-trip* clobber them.

---

## 7. Project scoping — now a requirement, not a preference

This section replaces the passing mentions of `project_id` in §1.1 and §1.4 with an explicit
demand, because the client-side situation is worse than the original document implied.

### 7.1 The gap: knowledge pull does no project filtering at all

Every other entity type is filtered **twice** — the pull URL fails closed when the project id
cannot be resolved (`pull.rs:38-62`), and then *every returned row* is re-checked against the
resolved id by `entity_matches_project` (`pull.rs:73-125`), applied per type at `pull.rs:259`
(entry), `:284` (task), `:319` (rule), `:344` (skill), `:377` (spec), `:406` (event),
`:427` (prompt), `:451` (file_change), `:479` (commit_link).

**Knowledge pages get the first guard and not the second.** `pull_knowledge_pages` builds the
scoped URL and fails closed (`knowledge.rs:225-229`) — then **discards the resolved id**:
`let (url, _project_id) = …` (`knowledge.rs:228`). The apply loop (`knowledge.rs:257-273`)
calls `apply_knowledge_record` with **no `entity_matches_project` call anywhere in the file**.
`KnowledgePageRecord` does carry `project_canonical_id` (`knowledge.rs:68-69`), populated on
push (`:87`) — but on the pull side it is deserialized and **never read**.

The consequence, stated plainly:

> **A foreign-project page that the server returns today is written straight to `cas.db` and
> to a markdown file on disk, with zero client-side detection — and it is merged on
> `rel_path` (`knowledge_store.rs:934`), so a foreign `architecture/build-system.md`
> silently overwrites this project's page of the same name unless that page is locked.**

It is also **unattributable after the fact**: `knowledge_pages` has no project column
(`knowledge_store.rs:57-96`), and the existing forensic tool `cas doctor --foreign-rows`
works only for tasks, by cross-referencing local-only *task* activity tables
(`cas-cli/src/cli/foreign_rows.rs:27-29`, `:48-60`). There is no equivalent evidence trail
for pages. **On this path the server is the only line of defence.**

We are fixing the client half. We are asking you to fix the server half, because the incidents
below all happened *with* client-side filtering present somewhere.

### 7.2 Why this is not hypothetical — four documented incidents

1. **The unscoped-pull leak (cas-ed15).** `/api/sync/pull` was called with no `project_id`
   and every `team_id IS NULL` row of the entire account was upserted into whichever local
   database was open. Blast radius: 14 databases; `cas-src` held 2,672 tasks of which 1,525
   were also in global (57%); another project measured 94% overlap; one 16-task set appeared
   in **10 different databases** with byte-identical `SUM(LENGTH(notes)) = 756`
   (`docs/reports/2026-08-03-task-store-contamination-cas-de89.md:9-61`,
   `docs/requests/BUG-cross-project-task-replication-2026-08-06.md:20-46`).
   Root cause, in the report's words: *"Cassy task scope is determined by the database opened
   by the caller. It is not stored as task provenance"* (`:9-13`) — **which is exactly the
   situation `knowledge_pages` is in today.**
2. **Canonical-id collision.** Two different clients' books —
   `…/Petra Stella/Accounting` and `…/Richards LLC/Accounting` — both resolved to the
   canonical id `"Accounting"` and merged into each other on every sync
   (`BUG-…:69-96`). The load-bearing sentence for your design: *"**Project-scoped pull does
   not help when two projects claim the same scope** — they will keep merging into each other
   on every sync"* (`:91-92`).
3. **Ids genuinely collide, so cleanup is hard.** Across 39 databases and 5,824 distinct ids,
   2,265 ids appeared in more than one database: 2,149 genuine replicas but **73 pure
   collisions** — different records sharing an id (`BUG-…:185-198`). Conclusion: *"Any
   id-keyed purge deletes real work. The key must be `(id, title)`"* (`:206`). Manual
   remediation removed 13,704 rows across 13 databases (`:224-240`).
4. **The migration will not carry the mess forward.** The mapping spec orders 11 stranded
   `sync_queue` entry rows to be drained or invalidated, never preserved, because replaying
   them post-migration would *"re-creat[e] exactly the cross-project contamination already
   visible in this corpus"* (spec:265-280). The reference corpus itself contains a memory
   titled *"Cross-project task contamination via cloud sync — root cause traced"*, and 41 of
   210 high-importance rows in one project database belong to a **different** project
   (spec:133-143).

### 7.3 What we are asking for

1. **A pull must never return a row outside the requested project scope.** Enforce it
   server-side, before the SELECT, exactly as was already done for the `project_id` parameter
   (`docs/requests/completed/SHIPPED-pull-endpoints-require-project-id.md:20-24` — 400 on a
   missing/whitespace `project_id`, *"returned before any SELECT (zero rows can leak)"*).
   Extend that guarantee to `types=knowledge_pages`.
2. **Finish the two items that shipped work left open** (same doc, `:40-44`): `NOT NULL` on
   `sync_entities.project_id` was deliberately not applied, and case-variant project ids
   (`Accounting` vs `accounting`) are still unnormalized. The client compares with exact `==`
   (`pull.rs:106`), so a case-variant row is silently *dropped* client-side — not a leak, but
   silent data loss.
3. **Tell us what you do about incident 2.** Two distinct projects can still claim one
   canonical id. The client now derives the id from the git remote ahead of the folder name
   (`cloud/config.rs:122-134`, v2.47.0), which fixes new installs, but the server holds the
   buckets that already merged.

---

## 8. The migration window: entities and pages carry the same memories

For a period, the same underlying memory exists **twice** on your side, and nothing in the
client relates the two.

- **Entities and pages are pushed by different code paths, in the same command.** Entries go
  out via `execute_push` (`cloud.rs:1569`), which reads the **entire corpus every run**
  (`store.list()`, `cloud.rs:1600-1605`) — no watermark, no queue. Pages go out via
  `sync_project_knowledge` (`cloud.rs:1123`), watermarked (§1.3), and called **last and
  non-fatally** (`cloud.rs:2396-2400`).
- **There is no correlation field on the wire.** Entry ids are `p-2025-01-01-001`-shaped
  (`crates/cas-types/src/entry.rs:225-226`); page ids are `cas-kn001`-shaped. No join table,
  no cross-reference column. `cas_legacy_id` (§6.1) is the *only* thing that will ever link
  them — and it lives in the page **body**, not in an indexed field.
- **The client cannot dedupe them.** They arrive by different requests into different local
  stores and surface through different retrieval channels, which are then blended by the
  hybrid scorer. The user-visible symptom is **the same fact appearing twice in one result
  list**, and at SessionStart, consuming context budget twice.
- **The asymmetry that will bite you:** because the entity path re-pushes the whole corpus on
  every sync while a page is pushed only when it changes, **any server-side "last write wins
  on arrival time" systematically favours the legacy entry over the migrated page.** If you
  rank, dedupe or expire by arrival time, the migration will appear to lose.
- **Deletion is already asymmetric.** Entries have `DELETE /api/sync/entries/{id}`
  (`push.rs:360-365`) and a team equivalent (`team_push.rs:521-524`). **Pages have no
  deletion path at all** — `apply_knowledge_record` always builds
  `tombstones: Vec::new()` (`knowledge.rs:293-298`). Worse, entry deletes travel *only* on the
  queue-driven daemon path; a user running `cas cloud sync` with no daemon deletes locally and
  the cloud row lives forever (`execute_push` has no delete handling).

We are not asking you to solve this yet — see the questions in §11.3–§11.5. We are asking you
not to design something that assumes one memory has one representation.

---

## 9. Do not bake in a snapshot-only *or* an offset-based assumption

GH #158 reports that the client's `sync_queue` is **write-only**: 73 rows on one machine,
oldest 2026-04-24, 0 failed — i.e. never attempted. The cause is that the queue is drained by
the daemon path (`push.rs:29`) and the team path, but **not** by `cas cloud push` /
`cas cloud sync`, which is what humans run (`execute_push`, snapshot).

**Status: explicitly undecided.** The tracking task (cas-bef4, P3) has a two-branch plan and
no code: either push consumes the queue, **or** the producers and table are removed by
migration because push is meant to stay snapshot-based. **Snapshot-forever is a live,
endorsed outcome — please do not plan around queue-driven sync as though it were scheduled.**

The constraint that survives either outcome:

> Design `/api/sync/push` so that (i) it is **idempotent per `(entity_type, entity_id)`
> regardless of how many times the same row arrives** — the snapshot path re-sends the entire
> corpus on every invocation today, so this is already load-bearing — and (ii) it **requires
> no positional token from the client on push**. The only client→server position that exists
> anywhere today is `since=` on *pull*, and its value is a string the *server itself* supplied
> via `pulled_at` (§1.3 ⟨A⟩).

Do not design a push API around a client-supplied offset or sequence number. The local queue
is keyed `UNIQUE(entity_type, entity_id, team_id)` with an `INTEGER PRIMARY KEY AUTOINCREMENT`
that is per-machine and per-root (`cloud/sync_queue/schema.rs:7-27`); it collapses repeated
writes to one row and deletes on success. It is a **pending-set, not an ordered log** — there
is no position in a stream for a client to name, and there is no proposal to create one.

---

## 10. Explicitly OUT of scope for the server

Listed because their absence from the original document could reasonably be read as an
invitation. These are client-owned and the client does not want server behaviour here:

| Not your problem | Why, and who owns it |
|---|---|
| **SessionStart index assembly** | Which pages are injected into an agent's opening context, in what order and within what budget, is a local retrieval decision made with no network. Local must work offline. |
| **Quarantine policy** | The migration quarantines suspect legacy rows by heuristic (mapping spec §4.1, spec:173-186). Which memories are junk is a client judgement about a client corpus. |
| **Distillation** | What becomes a page, its `page_type`, `title`, `rel_path` and `snippet`, is produced locally. The server never authors or rewrites page content. |
| **Slug / `rel_path` computation** | Stated in §1.5 and worth repeating: the receiver must take `rel_path` verbatim. Recomputing it forks one page into two across client versions. |
| **The `locked` decision** | Only the user sets it (`set_locked`, `knowledge_store.rs:1144`). The server *enforces* it (§3.1) but never *decides* it. |
| **Vector fan-out** | Every machine embeds its own pulled pages (§1.5). Not an optimization to add unilaterally — vectors are only comparable within one `(provider, model, dims)` space. |
| **Conflict merging** | Last-writer-wins on `updated_at` (§3.2), except `locked`. No three-way merge, no revision graph. |

---

## 11. Additional open questions (2026-08-07)

Numbered separately from §5, which stands. **§5.3 (opaque cursor) is now answered** — see
§1.3 ⟨A⟩; `pulled_at` already exists and knowledge pull should adopt it. §5.8 (deletion) is
**still open and now sharper** — see §8.

1. **Capability discovery.** There is none, anywhere: no `/api/capabilities`, no `features`
   key read from `/api/me` (`me.rs:92-104`), no client config flag. "Capability-gated"
   (cas-1ac6) means only `is_logged_in()` — a non-empty token
   (`embeddings.rs:118-130`, `config.rs:888-890`). **Consequence:** a fleet of logged-in
   clients pointed at a cloud with no `/api/embeddings` will emit an unbounded, permanently
   retrying stream of `POST /api/embeddings` — up to 32 inputs per call, per sync, per
   machine — and **no user will ever see an error** (`cloud.rs:1169-1182`, non-fatal warn;
   pages stay `pending_embedding = 1` and retry forever). Do you want a discovery endpoint?
   Note it requires a client change; the client will not use it until then.
2. **Will `cas-embed-v1` hold?** Given §2.3 ⟨A⟩ — the model string is a compile-time constant
   — can you commit to holding it stable across the whole supported client-version window,
   and to renaming rather than silently upgrading?
3. **Should the server dedupe an `entries` row against a `knowledge_pages` row?** The client
   cannot (§8). If it should happen at all it must be server-side, and it needs a correlation
   field — presumably `cas_legacy_id`, which today is only readable by parsing the page body.
   Would you want it promoted to a real wire field and indexed?
4. **Page tombstones, now that the asymmetry is visible.** Entries have DELETE routes; pages
   have nothing. Should `/api/sync` carry knowledge tombstones, under what key, and what
   should happen to a teammate's already-pulled copy?
5. **Orphaned entry rows.** Entry deletes are lost entirely on the CLI (non-daemon) path.
   Do you need a reconciliation or expiry story, or is unbounded retention acceptable?
6. **How is a `global`-scope page addressed on the wire?** The migration writes global
   memories into the **global root's** knowledge store and project memories into the
   project's (`memory_migration/mod.rs:63-67`), preserving the scope split. But the sync
   layer *requires* `project_canonical_id` and "the client refuses to sync outside a Cassy
   project" (§1.1) — and there is no global counterpart to a canonical id anywhere. **Today,
   global-scope pages appear to have no sync identity at all.** Is a global/account-scoped
   bucket something you can express? We are raising this as a question because we do not have
   an answer, not proposing a design.
7. **Do `cas_legacy_*` keys survive a round trip?** §6.2 asks you to treat `body` as opaque.
   Please confirm that nothing in your storage or transport path re-serializes markdown or
   YAML, and that a pushed body is byte-identical on pull.
8. **Test-fixture pollution.** 994 of 1,696 rows in the reference corpus are five literal
   integration-test fixture strings that were written into **real production databases**
   (spec:111-128). If any of those already reached the cloud, that is a server-side
   data-hygiene item, and we would like to know how to identify and remove them.

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

### Added by the 2026-08-07 amendment

| File | What it defines |
|---|---|
| `cas-cli/src/cloud/syncer/pull.rs:73-125` | `entity_matches_project` — the per-row project filter every entity type gets and knowledge pages do **not** (§7.1). |
| `cas-cli/src/cloud/syncer/pull.rs:505-508`, `:1061-1062` | The `pulled_at` echo protocol already in use for entity and team pull (§1.3 ⟨A⟩). |
| `crates/cas-store/src/knowledge_store.rs:57-96` | `knowledge_pages` DDL — note the absence of any project, team, share or tombstone column (§7.1). |
| `cas-cli/src/knowledge/merge.rs:92`, `:116-165`, `:186-203` | The hand-rolled frontmatter reader/writer and its owned-key set — why bodies must not be re-serialized (§6.2). |
| `cas-cli/src/cloud/embeddings.rs:118-130` | The whole of "capability-gated": `is_logged_in()` (§11.1). |
| `cas-cli/src/cli/cloud.rs:1569-1836` | `execute_push` — the snapshot path that re-sends the entire corpus every run (§8, §9). |
| `cas-cli/src/cloud/sync_queue/schema.rs:7-27` | The local queue: a pending-set keyed by entity, not an ordered log (§9). |
| `docs/reports/2026-08-03-task-store-contamination-cas-de89.md` | Incident 1 — the unscoped-pull leak and its blast radius (§7.2). |
| `docs/requests/BUG-cross-project-task-replication-2026-08-06.md` | Incidents 2–3 — canonical-id collision and colliding ids (§7.2). |
| `docs/requests/completed/SHIPPED-pull-endpoints-require-project-id.md` | The server-side scoping work that already shipped, and the two items it left open (§7.3). |

**On the cas-b129 migration branch** (`epic/memory-knowledge-migration-carry-the-legacy-memory-cas-b129`),
not yet merged — read with `git show <branch>:<path>`:

| File | What it defines |
|---|---|
| `docs/migration/cas-b129-mapping-spec.md` (added by `7885ed2f`) | Normative. Rules C1–C4 (frontmatter carriage), S1–S2 (inert `team_id`), L1–L2 (locking), P1–P2 (provenance); the corpus survey and the contamination evidence in §3. |
| `cas-cli/src/memory_migration/frontmatter.rs:26-52`, `:76-84`, `:145-202` | The 27 `cas_legacy_*` keys, the scope derivation, and the omit-when-default emitter (§6.1). |
| `cas-cli/src/memory_migration/mod.rs:63-67` | The per-root destination split: global memories → the global root's knowledge store (§11.6). |
