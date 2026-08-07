---
from: Petra Stella Cloud team
to: CAS CLI team
date: 2026-08-07
priority: P1
cas_task: cas-369a
re: docs/requests/2026-08-06-cloud-knowledge-sync-and-embeddings.md (incl. the 2026-08-07 amendment, §6–§11)
status: COMPLETE — all eight original questions and all eight amendment questions answered.
---

> **Disposition (2026-08-07, cas-ab75):** Reply, not a report — archived per `docs/requests/README.md`. Responds to `2026-08-06-cloud-knowledge-sync-and-embeddings.md`, archived alongside it.

# Response: server-side knowledge-page sync + the embeddings endpoint

Thank you for the transcription — building against pinned struct fields and serde
attributes rather than a sketch is the reason this landed without a round-trip. Two places
where it changed what we built are called out below (§Q1 and §What we found).

**Shipped:** the `/api/sync` half — push (§1.1–1.3) and pull (§1.4) of `knowledge_pages`,
including the §3.1 locked-page guard on the server side — **and `POST /api/embeddings`
(§2), now live in production.** Questions 4, 5 and 6 are answered from what actually
shipped, with numbers measured against the live endpoint rather than intended.

This document also answers the eight new questions in the 2026-08-07 amendment (§11) and
confirms compliance, with citations, against the amendment's hard requirements (§6.2, §6.3,
§7.3).

Server-side references are to `petra-stella-cloud`.

---

## What we found while implementing (two things you should know)

**1. The knowledge pull reaches the personal route, and that route could not have served a
teammate's page.** `pull_knowledge_pages` builds its URL through `build_scoped_pull_url`
(`knowledge.rs:221-237`), so it hits `/api/sync/pull` even when a team is configured —
there is no team path on that call. Our personal pull route filtered
`user_id = <caller> AND team_id IS NULL`. A naive "add `knowledge_page` to the pull list"
implementation would have returned `200` with a correctly-named, correctly-shaped
`knowledge_pages` array that **could never contain another account's page**. That is the
failure mode your §1.4 warns about, one layer below the envelope key: not a wrong key, a
right key over a query that structurally excludes teammates. Knowledge now runs its own
membership-aware visibility query on that route (`fetchVisibleKnowledgePages`,
`lib/knowledge.ts`).

**2. `locked` cost us a cross-row lookup, not a column check.** `sync_entities` is keyed
`(user_id, entity_type, id)`, so a teammate's copy of page `cas-kn001` is a *different row*
from yours. Enforcing "must not overwrite a locked page" against the pusher's own row would
have enforced nothing. The guard queries sibling rows sharing `(id, project_id, team_id)`
instead. Details in Q2.

---

## The eight questions

### Q1. Auth scoping — account, org, or team? What gates pull visibility?

**A token is scoped to an account.** API keys are per-user (`psc_k{n}_{hex}`, `lib/auth.ts`);
there is no org-scoped or team-scoped token. `validateToken` resolves a bearer token to one
user, and everything else is derived from that identity.

**Pull visibility for knowledge is the union of two sets**, both filtered by
`project_id` (`fetchVisibleKnowledgePages`, `lib/knowledge.ts`):

1. **your own pages**, at any `share` value;
2. **other accounts' pages with `share = "team"`**, in teams you are a member of —
   membership read live from `team_members`.

`share: "private"`, and an absent `share` (which we read as `private`, per §1.2), **never
cross accounts**. Test: *"keeps a teammate's private page out of the caller's pull"*,
`tests/api/sync/pull-knowledge-pages.test.ts`.

So: `team_id` gates *cross-account* visibility, `project_id` gates *which project's* pages,
and neither alone is sufficient — a row must clear both.

**Your sub-question — a user in two teams that share one `project_canonical_id` — has an
answer you will not like, and we would rather state it plainly than let you discover it.**
That user sees the **union** of both teams' team-shared pages for that canonical id, merged
into one list. The memberships query matches `team_id IN (all my teams)`, and
`project_canonical_id` is a git-remote-shaped string, not a tenant boundary. Two
consequences:

- Pages from team A and team B arrive in the same pull and are indistinguishable to the
  client — `team_id` is echoed per record, but §1.2 has no field for it, so the client
  discards it.
- Worse: if both teams hold a page with the **same id**, our dedupe picks one winner across
  the merged set. A team-B page can therefore become the canonical body for a page id you
  think of as team-A's.

This is a real cross-team bleed, bounded to users who are in multiple teams that share a
canonical project id. We believe that set is currently empty in practice, which is why it
is documented rather than hot-fixed. **The clean fix needs a decision from you:** either the
client sends its active `team_id` on the knowledge pull (small client change, exact fix), or
the server picks one team per pull by some rule we would be inventing on your behalf. We
would rather you choose. Until then, treat single-team membership as the supported
configuration for knowledge sync.

### Q2. `share` durability — retraction, or point-in-time instruction?

**As implemented, `share` is a durable property of the stored row that is evaluated at read
time — which behaves as a point-in-time distribution instruction, and does not support
retraction.**

Concretely: the value you send is normalized and stored in the row's payload
(`normalizeKnowledgePage`, `lib/knowledge.ts`) and every pull re-evaluates it. So changing
`share` on a later push *does* change visibility from that moment on — it is not frozen at
first write. But because the client derives `share` from "is a team configured *right now*"
(`knowledge_share_scope`, `knowledge.rs:135-141`) and, as you note, cannot re-push a
retraction, a page pushed as `team` stays `team` on the server until *something* pushes it
again with `share: "private"`.

**Unlinking a project from a team retracts nothing today.** We did not build automatic
retraction, deliberately: the server cannot distinguish "user left the team" from "user is
temporarily working outside the team context", and guessing wrong in the retracting
direction silently breaks a working team, while guessing wrong in the permissive direction
leaves data visible to people who were legitimately shown it earlier.

If you want retraction, the cheapest correct version is a client change: on unlink, re-push
affected pages once with `share: "private"`. That reuses the shipped path and needs nothing
from us. A server-side sweep ("when membership ends, flip that user's rows to private") is
possible but we would want it stated as a product requirement first — it is a data-deletion
behaviour, not a sync detail.

### Q3. Clock skew and `since` — opaque cursor?

**Shipped: `since` is applied leniently, as `updated_at >= since - 5 minutes`**
(`SINCE_SKEW_MS`, `lib/knowledge.ts`). Your §3.3 makes the trade explicit — redelivery is
idempotent, a skipped page is invisible until its source is edited again — so we widen the
window rather than narrow it. Five minutes covers ordinary NTP drift and a suspended
laptop without making routine incremental pulls re-return the corpus.

**Also: an unparseable or empty `since` is treated as *no filter*, not as a `400`.** A
malformed watermark should cost a client one oversized pull, not every page it has not yet
received. Test: *"does not fail a pull carrying a malformed since"*.

Note this leniency applies to the **knowledge** path only. The generic sync pull keeps its
existing strict `updated_at > since`; we did not want to change redelivery volume for every
other entity type as a side effect of this feature.

**On your proposal: yes, an opaque cursor is the right durable fix, and we would implement
the server half.** Wall-clock watermarks cannot be made correct by any amount of
server-side skew allowance — the allowance is a heuristic that trades bandwidth for safety,
and picking `5 minutes` is exactly the arbitrary constant that reveals the design is
approximate. A server-issued cursor the client echoes back removes the client's clock from
the protocol entirely. Since it is a client change, you own the timing; you are right that
it is much cheaper now than after there are many installs in the field. If you want it, we
suggest the server return `next_cursor` alongside `knowledge_pages` and accept
`cursor=<opaque>` in place of `since=`, supporting both during migration.

**Your §11 note that §5.3 is already answered is correct, and applies more widely than you
wrote.** Both of our pull routes already return `pulled_at` (`app/api/sync/pull/route.ts`,
`app/api/teams/[teamId]/sync/pull/route.ts`) — knowledge pull is the outlier on your side,
not a missing feature on ours. Adopting the existing `pulled_at` echo for knowledge is a
pure client change and needs nothing from us.

### Q4. Rate limits and the retry signal

All limits are **per account, sliding window** (`lib/rate-limit.ts`), and exceeding any of
them returns **`429` with a `Retry-After` header in seconds** — the signal you asked about,
so your existing push backoff is correct as written.

| Endpoint | Limit |
|---|---|
| `POST /api/sync/push` | 300 requests / 60s |
| `GET /api/sync/pull` | 120 requests / 60s |
| `POST /api/embeddings` | 120 requests / 60s |

The embeddings limit is deliberately generous relative to your batch size: at 32 inputs per
call that is 3,840 texts per minute per account, so a first-run index of a large repo is
bounded by the provider, not by us.

Your note that the client does **not** retry embeddings within a run — it leaves pages
`pending_embedding` for the next run — is understood and, we think, the right default. It is
also what makes a `429` safe for us to send: nothing spins, the work simply lands on the
next sync.

Also relevant to your §1.5 capacity note, since it bites before any rate limit does — push
body caps (`lib/gzip.ts`): **4 MB compressed, 20 MB decompressed**, exceeded → `413`. Page
bodies travel inline and gzipped, so a first-run push on a large repo is the realistic way
to hit this. The client chunks personal pushes per entity type, which keeps knowledge under
the cap in every case we have seen. The same caps apply to `POST /api/embeddings`, which is
far below them at 32 inputs.

### Q5. Embedding model, dimensionality, normalization

**Decided, shipped and live.**

| | |
|---|---|
| Wire model name | `cas-embed-v1` |
| Provider | OpenAI |
| Provider model | `text-embedding-3-large` |
| Dimensions | **1024** (requested via OpenAI's `dimensions` request parameter) |
| Normalization | **Unit L2 norm**, passed through verbatim — we do not renormalize |

Your `cas-embed-v1` / `1024` defaults are therefore **exactly right, and are now the
contract** — no client config change, and no cache wipe on upgrade.

**On normalization, measured rather than assumed.** We embedded two strings through the live
production endpoint and computed the L2 norm of each returned vector: **0.999671 and
0.999738**. Straight from the provider, before our JSON round trip, the same vectors measure
1.000212 and 1.000294. So: the vectors are unit-norm to within roughly ±3×10⁻⁴, and the
residual is float32 and JSON-decimal rounding, not a semantic difference. We apply no
normalization of our own in either direction — what the provider returns is what you get.

Practical advice: treat them as normalized (cosine and dot product agree to that tolerance),
but do not assert exact `1.0` anywhere, because no float pipeline will give you that.

**The stability commitment, restated as a commitment rather than an intention** (this is also
the answer to §11.2): **we will not change the model, dimensionality or normalization behind
the string `cas-embed-v1`.** Your §2.3 cache-wipe behaviour is correct and we will not
undermine it. Any model change ships as a **new** `model` identifier, coordinated with you as
a client release — we understand from §2.3 ⟨A⟩ that the name is a compile-time constant in
your binaries, so a rename is a release, not a config edit. We would rather pay one declared
re-embed per machine than leave a fleet holding silently incomparable vectors. Unknown model
strings are rejected with `400` rather than silently substituted, so a stale or typo'd name
fails loudly instead of quietly poisoning a cache.

**We persist no vectors.** `POST /api/embeddings` embeds the text in the request and returns
it; nothing is stored, cached or logged server-side beyond request metadata (counts, model,
duration). Per your §1.5, the vectors are yours to cache.

### Q6. Response shape — pick one

**We return the flat shape. You can retire the other parser.**

```json
{ "embeddings": [[0.1, 0.2, ...], [0.3, 0.4, ...]] }
```

We will not emit `{"data":[{"embedding":[...]}]}` from this endpoint. We chose the flat shape
precisely because it is *not* the provider's — it keeps our wire format independent of who
backs the endpoint, so a future provider change cannot leak into your parser. Transforming
the provider's response into it is where we enforce the §2.4 invariants anyway (below), so
the shape and the guarantees ship together.

`parse_embedding_response` (`embeddings.rs:220-236`) can drop its `data[].embedding` branch
whenever it suits your release schedule; there is no hurry from our side, and keeping both
costs you nothing.

**How we meet the §2.4 invariants**, since they were the substance behind the shape question:

- **Order.** We reassemble the response by the provider's `data[].index` explicitly, never by
  array position, and reject a duplicate or out-of-range index with `502`. Positional zipping
  (`embeddings.rs:505`) is therefore safe.
- **Count.** `vectors.len() != inputs.len()` never reaches you: a count mismatch is a `502`.
- **Never a zero vector in a `200`.** Every vector is checked for all-zero and for
  emptiness before the response is built; either one is a `502` with a logged
  `errorClass=zero_vector`. Your `rejected_zero` counter should never fire against us, and if
  it does we want to hear about it.
- **Dimension.** Every vector is length-checked against 1024 individually; a mismatch is a
  `502` (`errorClass=rejected_dims`), never a short vector in a `200`.
- **Upstream failure is an error status, never a soft-failed row.** Provider non-2xx,
  malformed body, unreachable provider, timeout (we allow 25s, inside your 30s client
  timeout) and missing server credentials all produce `502` or `503`.

Client-error cases, for completeness: unknown `model` → `400`; more than 32 inputs → `400`;
`input` absent, empty, non-array, non-string or blank-string → `400`; missing/invalid bearer
token → `401`; over the rate limit → `429` + `Retry-After`.

**The response body carries exactly one key.** A `200` is `{"embeddings": [...]}` and nothing
else — no `model` echo, no `usage`, no provider metadata. If you ever see another key, it is a
bug on our side, not an extension.

**Verified against production**, not against a mock, by two people independently: two inputs
returned two 1024-dim non-zero vectors in order, under a single `embeddings` key;
`cas-embed-v9000` returned `400 {"error":"Unknown model: cas-embed-v9000"}`; 33 inputs
returned `400 {"error":"input may contain at most 32 strings"}`; an unauthenticated call
returned `401`.

### Q7. Tenancy and body storage — where do the bodies live, for how long, and what else touches them?

This is the answer for the customer-facing version of the question.

**Where.** In one table, `sync_entities`, as a JSONB payload — the same table every other
CAS entity uses, keyed `(user_id, entity_type, id)` with `entity_type = 'knowledge_page'`
(`drizzle/schema.ts`). Scoping lives in real columns (`user_id`, `team_id`, `project_id`)
which are authoritative; the JSONB blob stays an opaque payload the server does not
interpret, with two deliberate exceptions: `locked` and `share`, which are read to enforce
§3.1 and visibility respectively. Both are top-level wire fields — **we never parse the body
to find them** (see §6.2/§6.3 compliance below). The store is Neon Postgres; the application
runs on Vercel. Page bodies are stored **as sent** — including `rel_path`, which we never
recompute (§1.4).

**For how long.** Indefinitely, until overwritten by a later push, or until explicitly
deleted (below). **There is no TTL and no retention sweep on knowledge pages.** The one
retention job we run (`app/api/cron/archive-events/route.ts`) is scoped to
`entity_type = 'event'` and does not touch knowledge rows.

**What else touches them.** Bodies are written by push and read back by pull. They are not
indexed, not scanned, and not used for analytics. Two other routes can reach them, both
generic per-entity CRUD that predates this work and accepts any `entity_type` string
without validation (`app/api/sync/[entityType]/[id]/route.ts`):

- `GET /api/sync/knowledge_page/<id>` — returns **your own** row's payload.
- `DELETE /api/sync/knowledge_page/<id>` — hard-deletes **your own** row. See Q8; this is
  not the tombstone path and you should not treat it as one.

Both are scoped `user_id = <caller>`, so neither reads nor removes another account's row.
Your CLI does not call either for knowledge today.

**`POST /api/embeddings` does not read stored pages.** It embeds the text in *your* request
body and returns the vectors; **no vector and no input text is persisted server-side**, and
we do not embed from storage. Request logging records counts, model name and duration — never
input text and never vector values.

**So the answer you can give a customer** is the same one you give today: local is the
source of truth, the cloud carries it. The server is a transport and a store, private
repository markdown is held as the account and its team scoped it, and nothing derives from
those bodies beyond returning them to the people entitled to see them.

### Q8. Deletion and tombstones

**No tombstone path exists, and we agree it is a real gap.** A page deleted locally survives
on the server and on every teammate's machine, exactly as you describe. Nothing in K1/K2
changes that.

One correction to the "there is simply no delete" framing, because it is worse than no
delete: a generic `DELETE /api/sync/{entityType}/{id}` **does** exist and does accept
`knowledge_page`. It hard-deletes the caller's own row only. That is not a usable deletion
path for a shared page, for two reasons worth stating explicitly:

- **It does not propagate.** No tombstone is recorded, so teammates who have already pulled
  keep their copies and never learn of the deletion.
- **It may not even take effect from the deleter's own point of view.** Because pull dedupes
  across per-account rows (K1's storage model), deleting *your* row of a team-shared page
  leaves teammates' rows intact — and your very next pull can re-deliver the page from one
  of them. Deleting a shared page by this route is, in the common case, a no-op with extra
  steps.

We are not proposing you use it. We are naming it so nobody builds "delete" on top of it. On
our side it is tracked as a bug (unvalidated `entity_type` on that route family, plus a real
decision about delete semantics for multi-row entities) and is gated on your answer here.

**We think it should be carried, and here is a concrete proposal to react to** — it is your
wire format, so this is a suggestion, not a decision:

- A `knowledge_tombstones` key in the **existing push envelope**, alongside
  `knowledge_pages`. Reusing the envelope means no new route, no new auth path, and it
  inherits gzip and the `skipped` reporting for free.
- Each element minimally `{ id, deleted_at }` — `id` because it is the identity §3.2 already
  uses, `deleted_at` because deletion has to participate in last-writer-wins or a delete and
  a concurrent edit cannot be ordered.
- Server behaviour: mark the row deleted rather than hard-deleting it (a hard delete cannot
  be transmitted to a teammate who has not pulled since), return tombstones under a
  `knowledge_tombstones` key on pull, and let `since` carry them like any other change.
- **The `locked` guard must apply to deletion too**, on the same reasoning as §3.1: if a
  teammate's tombstone can remove a page you locked, the lock stops meaning anything. We
  would refuse a cross-account tombstone against a locked page and report it in `skipped`,
  identically to the push guard.
- Retention of tombstones is the one genuinely open sub-question — they accumulate forever
  unless aged out, and ageing them out risks resurrection on a machine that was offline
  longer than the horizon. We would suggest a horizon well beyond any plausible offline
  period (a year) rather than trying to be clever.

Say the word and we will spec it properly; it is a small amount of server work sitting
behind a client-side wire decision.

---

# Answers to the 2026-08-07 amendment

## §11. The eight new questions

### §11.1 Capability discovery — do you want a discovery endpoint?

**You are right that there is none, and we are not going to pretend the instrumentation
substitutes for one.** For the specific failure you describe, though, the situation has
improved by construction: `POST /api/embeddings` now exists and is live, so a logged-in
client pointed at our cloud gets real vectors rather than an unbounded retry stream.

**Our answer: yes, we will build one if you want it, and we suggest you do — but it should
be `/api/me`, not a new endpoint.** You already call `/api/me` (`me.rs:92-104`), it is
already authenticated, and adding a `features` array to its response costs you one field
read instead of a new request in the sync path. A separate `/api/capabilities` would be a
second round trip on every startup to answer a question that changes about once a year.

Two honest caveats. First, as you note, it needs a client change, so it does nothing for
binaries already in the field — the fleet that would benefit most is exactly the one that
cannot use it. Second, a `features` list is only useful if you *act* on it; if the client
would log a warning and retry anyway, it buys nothing over the current behaviour.

**Meanwhile, we have instrumented on the assumption that our logs are your outage signal**
(your §2.4b). Every embeddings failure emits a structured log line carrying a stable
`errorClass` — `zero_vector`, `rejected_dims`, `count_mismatch`, `upstream_status`,
`upstream_timeout`, `upstream_unreachable`, `missing_api_key`, `bad_index`,
`duplicate_index`, `upstream_malformed` — plus the input and vector counts, the model, and
the duration. That is greppable per class in our log drain, which means we can see a
degradation that no user will ever report.

### §11.2 Will `cas-embed-v1` hold?

**Yes. Committed.** See Q5 for the full statement. In short: `cas-embed-v1` means OpenAI
`text-embedding-3-large` at 1024 dimensions, unit-norm, and it will keep meaning exactly that
across your whole supported client-version window. A change of model, dimensionality or
normalization ships under a **new** wire name, coordinated with you as a client release —
never as a silent upgrade behind the existing one. The mapping is a single pinned table in
`lib/embeddings.ts` with the rationale written above it, and unknown names `400` rather than
falling back to a default, so the failure mode of a mismatch is loud.

### §11.3 Should the server dedupe an `entries` row against a `knowledge_pages` row?

**We can, but not against a field we have to parse a markdown body to find — and we would
rather you did not ask us to.**

The blocker is exactly the one you name: `cas_legacy_id` lives inside the page `body`.
Reading it server-side would mean parsing the frontmatter, which is the one thing §6.2 tells
us never to do, and it would make our dedupe correctness depend on a hand-rolled
client-side format. We are not willing to take that dependency for a convenience feature.

**If you want server-side dedupe, promote `cas_legacy_id` to a real wire field** on the page
record (optional, absent for non-migrated pages). Then it is an indexable column for us, the
body stays opaque, and the correlation is explicit rather than inferred. We would index it
and could then offer either a dedupe at read time or a reconciliation report.

**Our recommendation is to do neither yet, and here is why.** Dedupe means choosing a winner,
and a wrong winner deletes a memory. During the migration window the two representations are
not equivalent — the page has been rewritten by distillation, the entry has not — so "the
same fact appearing twice" is a *retrieval* problem before it is a *storage* problem, and
retrieval is yours (§10). If you promote the field, we would start with a read-only
reconciliation report (how many pairs exist, how they differ) before anything deletes
anything.

### §11.4 Page tombstones

**Answered in full at Q8**, including a concrete wire proposal and the `locked`-applies-to-
deletion requirement. Two additions in light of the amendment.

First, the asymmetry you note in §8 — entries have a DELETE route, pages have none — is real
but the entries route is not the model to copy. Our own audit found that the generic
`DELETE /api/sync/{entityType}/{id}` accepts **any** `entity_type` string with no validation,
including `knowledge_page`, and that using it on a shared page is worse than useless: it
removes only the caller's row, records no tombstone, and the next pull can re-deliver the
page from a teammate's row via cross-row dedupe. We have filed that on our side as a bug in
its own right — an `entity_type` allowlist on that route family, plus a real decision on
delete semantics for entities stored as multiple per-account rows. **The allowlist we will
fix regardless; the semantics wait on your tombstone answer**, because building delete
semantics before the wire format is decided would just be a second thing to undo.

Second, on your teammate's already-pulled copy: a tombstone that only reaches machines which
pull *after* it is recorded is the whole reason we propose a stored tombstone rather than a
hard delete. A hard delete is unobservable to anyone who has not pulled since.

### §11.5 Orphaned entry rows — reconciliation or expiry?

**Unbounded retention is what we do today, and it is acceptable to us at current volume — but
you should not rely on that as a policy, because it is a consequence rather than a decision.**

The facts: entry rows have no TTL and no expiry sweep. Our only retention job is scoped to
`entity_type = 'event'`. So an entry deleted locally on a non-daemon `cas cloud sync` — your
`execute_push` path with no delete handling — lives on our side forever, and because that
path re-pushes the whole corpus every run, it is also *re-asserted* on every sync by every
machine that still has it. Storage cost is not our concern at this scale; the concern is
that a "deleted" memory keeps being served back.

**We would rather fix this by making deletion expressible than by inventing expiry.** An
expiry horizon has the same resurrection problem as tombstone ageing (§Q8) and, worse, would
silently delete rows a user never asked to delete. If you want a reconciliation story, the
cheapest correct one reuses the snapshot property you already have: since `execute_push`
sends the *entire* corpus every run, a push could optionally declare itself authoritative for
a `(project, entity_type)` — and we would then treat absence as deletion for that scope. That
is a real design with real risk (a partial push under that flag would delete real work), so
we would want it explicitly opted into per request, never inferred. Tell us if it is worth
speccing.

### §11.6 How is a `global`-scope page addressed on the wire?

**It cannot be, today — and we have just made that harder rather than easier, deliberately.**

You are right that there is no global counterpart to a canonical id. On our side
`project_id` is now **`NOT NULL`** on `sync_entities` (migration 0013, live in production),
and both push paths already reject an empty `project_canonical_id` with `400` before any
write. So a page with no project identity has nowhere to go: it is not that we would store it
badly, it is that we would refuse it.

We think that is the right posture for now, because the alternative — a magic reserved
canonical id like `__global__` — creates a bucket that is account-wide, crosses every project
boundary, and would be indistinguishable from a real project whose folder happened to be
named that. Given the four incidents in §7.2 are all about content crossing a scope boundary,
inventing a scope that has no boundary seemed like the wrong direction to guess in.

**What we can offer, if you want account-scoped pages:** a first-class account scope, not a
sentinel string — i.e. a nullable `project_id` re-allowed *only* in combination with an
explicit `scope: "account"` on the wire, so the absence is declared rather than inferred, and
pull for account scope is an explicit request rather than a wildcard. That is a schema and
protocol change on both sides and we are not going to start it unilaterally. We agree with
your framing that this is a genuine open design question rather than a bug, and we are
answering it as one.

### §11.7 Do `cas_legacy_*` keys survive a round trip?

**Confirmed: yes, byte-for-byte. We audited every path and then built a regression lock so it
stays true.** This is also the §6.2 compliance answer.

The audit, in full: the body string is never parsed, transformed, trimmed, re-encoded, folded
or normalized anywhere on the server. `middleware.ts` is CORS-only and never touches a body.
`lib/gzip.ts readBody()` is a byte-preserving UTF-8 decode (both the plain and gzip branches).
`normalizeKnowledgePage`'s `asString` **returns the input string itself** for a string body —
a reference passthrough, not a rebuild. `toWireRecord` shallow-spreads it. `requestLogger`
emits counts and ids and never serializes `data`. The generic `/api/sync/{type}/{id}` GET is a
raw passthrough. No sanitization or normalization middleware exists anywhere in the repo.
There is exactly **one** mutation in the entire path — a non-string `body` falls back to `""`
— which is unreachable for a real client (you serialize `body` as a Rust `String`), replaces
rather than corrupts, and is pinned by its own test as a documented boundary.

**JSONB fidelity was verified against the live database, not assumed.** All fixtures
round-trip byte-identical with matching md5 through both `jsonb_build_object` and the
driver-realistic `('{"body":' || to_json(body) || '}')::jsonb ->> 'body'` path — including a
200,000-character unbroken line, CRLF, BOM plus lone CR plus trailing whitespace, and
astral/combining/ZWJ/bidi Unicode. JSONB's normalizations (key order, duplicate-key elision,
numeric canonicalization) are **structural** and provably cannot reach inside a string scalar.

**One real limitation, and it is not ours to fix:** a body containing `U+0000` cannot be
stored — Postgres rejects it with *"unsupported Unicode escape sequence"* (confirmed live).
That is a **loud** failure (the insert errors) rather than silent corruption, which is the
safe direction, and markdown bodies do not contain NUL. We have documented it in the test file
so that nobody later "fixes" it by stripping NULs — stripping would be the actual corruption.

**The regression lock:** `tests/api/sync/knowledge-body-roundtrip.test.ts`, 39 cases across
nine byte-fragile fixtures (including a real `cas_legacy_*` block with duplicate keys, an
unquoted colon and an empty value), driven push → stored blob → JSONB encode/decode → pull →
parsed response, asserting strict `===` **plus** `Buffer.compare === 0` **plus** equal
`.length`. Deliberately *not* deep-equal of parsed structures, which is precisely the
assertion a re-serialized body would pass. It also covers the gzip transport branch, the team
pull route, raw response-text escaping, and a three-cycle re-push fixed-point test, because
cumulative drift is the realistic failure mode and is invisible on one hop.

**The suite is provably load-bearing, not vacuous:** we mutation-tested it by temporarily
inserting `.replace(/\r\n/g,"\n").trim()` into the body line — 20 of the 39 cases went red.
Reverted immediately; no production code was changed by that work, because none needed to be.

One fixture detail worth flagging back to you, since it changed our test after we read your
emitter: `cas_legacy_tags` is the single list block Rule C4 permits, and
`memory_migration/frontmatter.rs:140-203` emits it as `cas_legacy_tags:` followed by
**two-space-indented `- item` lines**, with the optional keys after the block. Our original
fixture used an inline `['a','b','c']` — plausible, but not a shape that crosses the wire. It
mattered: those `- item` lines carry no colon, so they take your reader's passthrough branch
(`merge.rs:139-144`), and a list block is the construct a YAML emitter is most likely to
reflow. The fixture now matches your emitter exactly, including indentation, ordering, a
trailing-whitespace list item, and a re-quote-bait value.

### §11.8 Test-fixture pollution

**Give us the five literal strings and we will audit and remove them.**

We have not swept for them because we do not know what they are — and we are not going to
guess at a match pattern for a deletion that runs against production data. Send the exact
strings (or unambiguous prefixes) and we will run a read-only count first, report what we
find, broken down by account and project, and delete only after you confirm the match set
looks right. Deleting rows out of a live corpus on a pattern we inferred is precisely the
class of operation that turns a hygiene item into an incident.

Two notes on scope. First, these would be `entries`, not knowledge pages, on our side — so
this is a pre-existing data-hygiene item rather than anything the migration introduces.
Second, because the snapshot push path re-sends the whole corpus every run (§8, §9), deleting
them server-side does **not** make them stay deleted: any machine that still holds them
locally will re-push them on its next sync. The server-side sweep has to follow the local
cleanup, not precede it, or we will both watch the rows come back.

---

## Compliance with the amendment's hard requirements

### §6.2 — `body` is stored and returned as an opaque blob, never re-serialized

**Compliant, with proof.** Full audit and the 39-case regression lock are in §11.7 above.
`body` is a JSONB string scalar; no YAML or markdown parser exists anywhere in the request
path; the one mutation (non-string → `""`) is unreachable from a real client and pinned by a
test. Mutation-tested to confirm the suite would actually catch a future normalization.

### §6.3 — `cas_legacy_team_id` is inert; no `share` or ACL derived from it

**Compliant by construction, which is a stronger guarantee than compliant by policy.** The
server never parses the page body at all, so it cannot read `cas_legacy_team_id` even by
accident — there is no code path that could. The only fields we read from a page payload are
`locked` and `share`, both top-level wire fields per §1.2.

Visibility is derived from exactly two sources, neither of which is the body: the `team_id`
column on the row (set from the authenticated push context, not from page content) and live
`team_members` membership. So no share value, team membership, ACL, distribution list or
visibility default is or can be synthesized from any `cas_legacy_*` key. Your Rules S1/S2
describe what we already do.

We also note your E3 status accurately: we are treating it as an open escalation on your
side, not a ratified ruling, and nothing we built depends on how it resolves — because our
answer ("do nothing with the field") is the same either way.

### §7.3 item 1 — a pull never returns a row outside the requested project scope

**Compliant, enforced before the SELECT, and extended to `types=knowledge_pages`.**

`project_id` is mandatory on both pull routes: a missing or whitespace-only value returns
`400` **before any query runs**, so zero rows can leak (`app/api/sync/pull/route.ts`,
`app/api/teams/[teamId]/sync/pull/route.ts`). Every row-returning query — the generic entity
select and `fetchVisibleKnowledgePages` alike — carries `eq(syncEntities.projectId,
projectId)` as a filter, so the guarantee does not depend on the URL guard alone. The
knowledge visibility query applies it to **both** halves of the union (your own pages and
teammates' `share:"team"` pages), and `project_id` is echoed on every returned record so your
`entity_matches_project` re-check has something to compare against.

Tests in `tests/api/sync/pull-knowledge-pages.test.ts` cover the `400` on a missing
`project_id`, the echoed `project_id` on returned pages, and an escaped `owner/repo`-shaped id
passing through to the filter unmangled.

### §7.3 item 2 — `NOT NULL` on `project_id`, and case-variant canonical ids

**Item 2a — `NOT NULL`: shipped and live in production.** Migration
`0013_sync_entities_project_id_not_null.sql`, applied to the production database and verified
via `information_schema` (`is_nullable = 'NO'`).

The audit behind it, on the full production table (216,722 rows, 18 distinct projects, 6
entity types): **0 NULL, 0 empty-string, 0 untrimmed** `project_id`. So no backfill was
required and no per-type exemption was needed. Before applying it to production we applied it
to a copy-on-write clone of the production database carrying all rows, confirmed the catalog
flipped, then confirmed the constraint actually **bites** (a NULL insert was rejected; a
normal insert still succeeded), then destroyed the clone. A catalog flip alone would not have
been evidence.

Worth reporting because it may apply to your own schema discipline: this could not be
generated. `drizzle-kit generate` emitted *"No schema changes, nothing to migrate"*, because
our ORM schema **and** its snapshot both already claimed `notNull` — the drift was between
(schema + snapshot) and the live database, which still reported `is_nullable = YES`. We had
been asserting a constraint in TypeScript that Postgres never enforced. The migration is
hand-written for that reason.

Note this is defence in depth rather than a fix for a live leak: both push paths already
`400` on an empty `project_canonical_id`, so the app layer could not emit a NULL today. It
closes the gap against a *future* write path.

**Item 2b — case-variant canonical ids: we are deliberately doing nothing server-side, and
the reason inverts the option you might expect us to take.**

Our ruling: **no server-side case normalization. Matching stays byte-exact.** Canonical-id
normalization belongs client-side, at derivation.

We audited production and found two real case collisions, both single-user, both personal
(`team_id IS NULL`), and both content-verified as **one** logical project each rather than two
projects colliding — case drift from folder naming, not a tenancy problem:
`Accounting` (11,081 rows) + `accounting` (29 rows), and `Penguinz` (138) + `penguinz` (36).

**Then we found the thing that decides it.** Your client re-checks every returned row with a
byte-exact `s == current_project_id` (`entity_matches_project`, `pull.rs:73-125`) and drops
mismatches with only a stderr warning — and all three of our pull paths echo the **stored**
`project_id` back to you. So if we lowercased on write and echoed `accounting` to a client
whose derived canonical id is `Accounting`, that client would drop **100%** of its rows. The
"obvious" normalization converts today's partial loss (29 rows invisible) into a **total
blackout of 11,110 rows** for a live, active account. It is strictly worse than doing nothing,
and we are glad we measured before shipping it.

We also rejected rejecting mixed case (`400` on push), which would immediately break the
larger and more recent live bucket for a user who never chose that casing. And we rejected the
one technically viable variant — matching on `lower(project_id)` while echoing the caller's
requested casing — on principle rather than feasibility: it would have the server inventing
case-insensitive tenancy for every user and every entity type, and silently merging any future
*genuinely distinct* case-variant pair. That is incident 2 with our fingerprints on it. It is
the same test we applied to the cross-team-bleed question in Q1, and it gets the same answer:
**we do not invent scoping semantics on your behalf.**

**The root cause is in `resolve_canonical_id`** (`cloud/config.rs:117-131`): `config.toml` →
git remote → **parent folder name** → path hash, and nothing in that chain lowercases (your
only `to_lowercase`, `team_push.rs:52`, applies to the git *remote* for team resolution — a
different field). So casing follows the folder, and two machines with differently-cased
folders, or one folder rename, fork one project into two buckets. Only the client can know the
two spellings mean one project.

**The exact, reversible remedy is already in your hands:** pin `[project] canonical_id` in
`.cas/config.toml`, which is resolution step 1 and overrides folder-name derivation entirely
(`cas cloud project set` writes it). For the two existing collisions we have verified that
folding the smaller variant into the larger is conflict-free — the primary key is
`(user_id, entity_type, id)` and excludes `project_id`, so lowering cannot duplicate a key
(measured: 0 collisions for both groups). We have **not** run it. It is available on request,
and it should follow the config pin rather than substitute for it.

### §7.3 item 3 — what we do about incident 2 (two distinct projects, one canonical id)

**Partly good news, and one honest gap.**

**For team pushes, the server already resolves by normalized git remote and it wins over the
slug.** `resolveCanonicalProject` (`lib/projects.ts`, called from the team push route)
resolves in this order: normalized `git_remote` match against `projects.git_remote` → alias
match → `canonical_id` match (backfilling `git_remote` when the matched row lacks one) → insert
a new project. There are partial-unique indexes on `(team_id, canonical_id)` and
`(team_id, git_remote)`, and on conflict **the git-remote owner wins** rather than the sent
slug. We normalize remotes with the same rule you do — strip scheme, `git@host:` form, `.git`
suffix and trailing slash, then lowercase. So for team projects, two different repositories
whose folders both derive `"Accounting"` are separated by their remotes, which is exactly the
v2.47.0 client behaviour you describe, enforced server-side as well.

**The gap: the personal push path does not do this.** `app/api/sync/push/route.ts` stores the
`project_canonical_id` string you send, verbatim. It never consults `projects.git_remote`. And
both production collisions we found are personal rows, so the mechanism that would have helped
is not on the path where the problem actually lives.

**For buckets that already merged, we have no way to unmerge them and we will not guess.** Two
distinct projects sharing one canonical id are, by construction, indistinguishable in our data
— there is no per-row provenance recording which working copy a row came from. Splitting them
would require a discriminator we do not have.

**What we can do, and what we would need.** If you start sending the normalized `git_remote`
on personal pushes as well, we can (a) apply the same remote-first resolution there, which
prevents new merges, and (b) partition *newly arriving* rows correctly while leaving history
untouched. We would not retroactively rewrite existing rows on that basis without a per-case
review, because a wrong split loses work exactly as badly as a wrong merge. If you would like
that, it is a small client change and a moderate server change, and we would want it specified
before either of us starts.

### §8 — we do not use arrival-time last-write-wins

**Confirmed, and this was already true before you raised it.** Conflict resolution uses the
`updated_at` **carried in the record**, never the time the request arrived (§3.2). Knowledge
dedupe across per-account rows resolves: `locked` wins first, then newest `updated_at`, then
`user_id` ascending as a deterministic tiebreak (`dedupeKnowledgeRows`, `lib/knowledge.ts`).
Incremental pull filters on `updated_at`, not on insertion time.

So the asymmetry you warn about — the entity path re-pushing the whole corpus every run while
pages are watermarked — does **not** systematically favour the legacy entry over the migrated
page on our side. A re-pushed but unchanged entry carries its original `updated_at` and does
not win anything by arriving more often.

We have also taken your §9 constraint as binding: `/api/sync/push` is idempotent per
`(entity_type, entity_id)` regardless of how many times the same row arrives, and requires **no
positional token from the client**. The only client→server position anywhere in the protocol is
`since=` on pull, whose value we supplied ourselves via `pulled_at`. We are not designing
around a queue, an offset or a sequence number, and we have no plans that assume `sync_queue`
becomes queue-driven.

### §10 — out of scope

Acknowledged in full, and nothing we built crosses those lines. Specifically: we never author
or rewrite page content; we take `rel_path` verbatim and never recompute a slug; we enforce
`locked` but never set it; we do not embed pages server-side or fan vectors out to other
machines (every machine embeds its own pulls); we do not merge conflicts beyond last-writer-
wins on `updated_at` with the `locked` exception; and we make no decisions about SessionStart
assembly, quarantine or distillation.

---

## What shipped, for your reference

| Behaviour | Where |
|---|---|
| `knowledge_pages` accepted on push, stored as `entity_type = 'knowledge_page'` | `app/api/sync/push/route.ts`, `lib/entity-types.ts` |
| Record normalization — `rel_path` verbatim, absent `share` → `private`, `locked`/`snippet`/`body`/`sources` defaults, RFC 3339 timestamps | `normalizeKnowledgePage`, `lib/knowledge.ts` |
| §3.1 locked-page guard, across sibling rows, refusals dropped before write | `findLockedPageIds` / `partitionByLock`, `lib/knowledge.ts` |
| `skipped.knowledge_pages` in the push response | `app/api/sync/push/route.ts` |
| Pull under exactly the `knowledge_pages` key | `app/api/sync/pull/route.ts` |
| Visibility (own pages + teammates' `share: "team"`) | `fetchVisibleKnowledgePages`, `lib/knowledge.ts` |
| Per-user row dedupe — locked wins, else newest `updated_at`, else `user_id` ascending | `dedupeKnowledgeRows`, `lib/knowledge.ts` |
| Lenient `since` (`>= since - 5min`), malformed `since` ignored | `sinceLowerBound`, `lib/knowledge.ts` |
| **`POST /api/embeddings` — `cas-embed-v1` → OpenAI `text-embedding-3-large` @ 1024 dims, flat response shape** | `app/api/embeddings/route.ts`, `lib/embeddings.ts` |
| **§2.4 invariants — index-mapped ordering, count match, per-vector dimension and zero-vector rejection, upstream failure → `502`/`503`** | `normalizeProviderVectors`, `lib/embeddings.ts` |
| **`NOT NULL` on `sync_entities.project_id`** | `drizzle/migrations/0013_sync_entities_project_id_not_null.sql` (applied to production) |
| Tests — 13 push, 19 pull, 39 body byte-identity cases, 19 embeddings | `tests/api/sync/push-knowledge-pages.test.ts`, `…/pull-knowledge-pages.test.ts`, `…/knowledge-body-roundtrip.test.ts`, `tests/api/embeddings.test.ts` |

Two implementation notes that may matter to you:

- **`skipped.knowledge_pages` counts lock refusals only.** A last-writer-wins no-op — an
  unchanged re-push — reports `0`, not `1`. We read your §1.1 as wanting a conflict signal,
  and a counter that fires on every benign no-op would be one you learn to ignore.
- **`types=` is now honoured on pull.** Sending `types=knowledge_pages` narrows the response
  to that key; omitting `types` returns the legacy envelope byte-for-byte unchanged, and
  **without** a `knowledge_pages` key. Knowledge is opt-in because bodies are full markdown
  and folding them into every generic sync pull would multiply payload size for a caller
  that never reads the key. Your knowledge pull already sends `types` (`knowledge.rs:221`)
  and the main pull already sends `since` alone (`pull.rs:228-232`), so nothing on your side
  needs to change — but it is a behaviour difference worth knowing about.

---

## What we need back from you

Nothing here blocks you from using what shipped. These are the decisions only you can make,
in rough order of how much they cost you to leave open:

1. **Cross-team bleed (Q1).** Does the client send its active `team_id` on the knowledge
   pull? Until it does, single-team membership is the supported configuration.
2. **Tombstones (Q8 / §11.4).** React to the wire proposal, or tell us it stays out of scope.
   Our `entity_type` allowlist fix does not wait on you; the delete *semantics* do.
3. **`cas_legacy_id` as a wire field (§11.3).** Only needed if you want server-side dedupe;
   we recommend a reconciliation report before anything deletes.
4. **The five test-fixture strings (§11.8).** Send them and we will audit, report, then clean
   — after the local cleanup, or the snapshot push will re-create them.
5. **Global/account scope (§11.6).** A genuine open design question. Our current answer is
   that `project_id` is now `NOT NULL`, so account-scoped pages have no representation.
6. **Capability discovery (§11.1).** We suggest a `features` array on `/api/me` rather than a
   new endpoint. Yours to schedule; it needs a client release either way.
7. **Case-variant canonical ids (§7.3-2).** Pin `[project] canonical_id` on the affected
   machines. The one-off data fold for the two existing collisions is proven safe and waiting
   on your word.
