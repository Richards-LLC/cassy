---
from: CAS CLI team
to: Petra Stella Cloud team
date: 2026-08-07
priority: P1
cas_task: cas-ff97
re: docs/requests/RESPONSE-cloud-knowledge-sync-and-embeddings.md — "What we need back from you", items 1-6, plus the §7.3-3 joint spec
status: COMPLETE — all six open items answered; item 7 was answered and executed separately.
---

# Answers: the six open decisions, and the `git_remote`-on-personal-push spec

Your response was the most useful document either team has produced in this exchange, and
the two things you found while implementing — that the knowledge pull reaches the personal
route, and that `locked` needs a cross-row lookup — are both things we would not have
predicted from our own side of the wire. Thank you for measuring the normalization instead
of asserting it, and for refusing to invent scoping semantics on our behalf in §7.3-2b. The
"we do not invent scoping semantics for you" rule is the right one; this document is us
holding up our end of it.

Every position below is either **committed** with the CAS task that lands it named, or
**explicitly deferred** with the reason and the gate. Nothing is left silent. Where we
depart from your suggestion, we say so and why.

Client-side references are to `cas-src` at commit `7c326c43` unless marked otherwise. A few
citations are to `cas-cli/src/memory_migration/`, which lives only on the unmerged
`epic/memory-knowledge-migration-…-cas-b129` branch — those are marked **(b129)** so you
never go looking for a file that is not on `main`.

---

## Summary

| # | Your item | Our answer | Lands in | Unblocks |
|---|---|---|---|---|
| 1 | `team_id` on the knowledge pull | **Committed.** `team_id=` query param, single active team, absence keeps today's semantics | `cas-f177` | your `cas-4b16` |
| 2 | Tombstones | **Adopt your proposal**, scoped to knowledge pages; client-side deletes land after the memory migration settles | `cas-e6aa` | your `cas-e9e6` (delete-semantics half) |
| 3 | `cas_legacy_id` as a wire field | **No, this phase.** No server dedupe. Reconciliation report accepted in principle, gated on the same field | — (revisit post-migration) | — |
| 4 | The five fixture strings | **Sequenced, not withheld.** Local purge lands first, then we send the literals | `cas-78c8` (GH #156) | — |
| 5 | Global / account scope | **Explicitly deferred.** Global pages are unsynced *by design* this phase; the design question is real and ours to answer | `cas-a924` | — |
| 6 | Capability discovery | **Accepted in principle** — `features` array on `/api/me`, with three semantics we need from you | rides `cas-e000` | — |
| — | §7.3-3 `git_remote` on personal push | **Joint spec below (§7).** Yours to ack, then we build | `cas-7719` | your `cas-244f` |
| 7 | Case-variant canonical ids | Answered and executed — see `RESPONSE-canonical-id-fold-decision.md` in your inbox; both folds ran, receipts appended | closed | closed |

---

## 0. Since you shipped: the memory migration is live

Timing worth having in the same delivery, because it changes what your `knowledge_pages`
surface may see. **The `cas-b129` memory migration cut over against the real stores at
approximately 19:25Z today.** All seven verifications passed, including the V7 contamination
gate, which came back clean.

What that means concretely, on the one machine that has cut over so far:

- **146 knowledge pages now exist client-side** — 107 project-scope and 39 global-scope, of
  which **21 are locked carry-verbatim pages** (the rows whose bodies must survive
  byte-for-byte; your §11.7 round-trip lock is what protects those on the wire).
- **Your shipped knowledge-page sync surface may therefore start receiving real traffic**
  on future `cas cloud sync` runs. Every such run is operator-initiated and **none is
  scheduled**, so this is a heads-up rather than a warning: when the first real push lands
  it will not be a test fixture.
- The 11 stranded global `sync_queue` rows were **invalidated with full ledgered payloads**,
  per the fallback we had already agreed internally — nothing was dropped silently.

Two connections to items below, so the numbers are not just trivia:

- **Item 5 gets a real number.** 39 of those 146 pages are global-scope, and by the boundary
  described in §5 they have no sync identity at all — they exist locally and cannot be
  addressed on the wire. That is the size of the open design question today, on one machine.
- **Item 4 is unaffected and still sequenced.** The five fixture strings for your §11.8
  cleanup are deliberately **not** in this document; they follow separately once our local
  purge lands (`cas-78c8`). See §4 for the ordering and why it matters.

Note for anyone reading the citations: the migration ran from the `cas-b129` epic branch,
which is **still unmerged**. So every **(b129)** file reference below remains branch-only —
live behaviour and merged code are two different questions right now.

---

## 1. `team_id` on the knowledge pull — committed

**Yes. The client will send its active `team_id` on the knowledge pull.** Tracked as
**`cas-f177`** on epic `cas-e000`, related to `cas-2cc5` (the per-row refusal work). Until
it ships, your "single-team membership is the supported configuration" statement stands and
we will carry it in our own docs rather than let a user discover it.

### 1.1 Wire shape: a query parameter, not a header

Send `team_id=<id>` as a query parameter on `GET /api/sync/pull`.

Reasons, in the order they mattered to us:

- The knowledge pull's entire scope is already expressed in query params, built in exactly
  one place: `build_scoped_pull_url` appends `project_id=` (`syncer/pull.rs:38-62`) to the
  `types=` and `since=` params assembled by `pull_knowledge_pages`
  (`syncer/knowledge.rs:221-229`). Putting one dimension of scope in a header would split
  scope across two channels and make a request no longer reproducible from its own URL.
- Scope in the URL is greppable in your logs and in a `curl` repro. For a class of bug whose
  entire failure mode is "the wrong rows came back", that matters more than elegance.
- We already send `team_id` as a **body field** on push (`syncer/push.rs:200-203`,
  `push_sub_batch` at `:467-470`) and never as a header. A query param on a GET is the same
  rule — scope travels inside the request, in the same encoding as everything else in it.

`project_id` is percent-encoded for `/` today (`pull.rs:59`); team ids are opaque and we
will encode them the same way rather than assume they are URL-safe.

### 1.2 Which team, when the user is in several

**There is exactly one, always, by construction.** Our client has no concept of two
simultaneously-active teams: the active team is a single `Option<String>` resolved by
`CloudConfig::active_team_id()` (`cloud/config.rs:997-1000`, delegating to
`active_team_id_with_user_config` at `:940-996`):

0. `team_auto_promote = Some(false)` → `None`. A hard kill switch; nothing overrides it.
1. Project-level `team_id` pinned in the project's cloud config → that team wins.
2. Otherwise, user-level fallback (`default_team_id`, then a single-team auto-pick) applies
   **only** under an explicit `team_auto_promote = Some(true)` — the guard exists so a
   personal workspace is never silently promoted to team scope because the user happens to
   have a team configured elsewhere (`cas-f8e3`).

So the param is either one id or absent. This is not a new rule invented for the pull: it
is the same resolver the automatic daemon already uses to decide which team to push
(`syncer/pull.rs:798`).

**It must be this resolver and not the narrower project-level `team_id` field**, because the
push side already uses it: `knowledge_share_scope` derives a page's `share` from
`active_team_id().is_some()` (`syncer/knowledge.rs:135-141`, called at `:164`). If the pull
used a different notion of "my team" than the push, a machine could publish into one team's
set and read from another's — which is a harder bug to see than the one we are fixing.

### 1.3 Server semantics we are asking for

Four of these are decisions rather than preferences. We would rather over-specify here than
have you infer.

1. **`team_id` narrows only the cross-account half of the union.** The caller's own pages
   must keep coming back at any `share` value. A user's own `private` pages are not team
   data, and a user with two laptops on the same account relies on that half of the union to
   sync their own knowledge to themselves. Narrowing the whole union would silently break
   personal multi-machine sync the moment a project is linked to a team.
2. **`team_id` present but the caller is not a member → `403`, loudly.** Not an empty `200`,
   not a silent fall-back to the union. A wrong or stale team id must not be
   indistinguishable from "that team has no pages" — that is exactly the class of silent
   narrowing that made the knowledge pull gap survive as long as it did on our side.
3. **`team_id` absent → today's behaviour, byte for byte.** Every binary already in the
   field omits the param, and there is no client-side kill switch for a semantics change you
   make server-side. Absence must keep meaning "union across all my teams", *not* "personal
   only". This is a compatibility requirement, not a preference; if it is ever going to
   change, it changes with a version negotiation, not a deploy.
4. **Unknown or malformed extra params stay non-fatal**, matching the posture you already
   took with a malformed `since` (your Q3). A client that sends `team_id=` empty should get
   the absent-param behaviour, not a `400`.

One small thing that is already right and we would ask you not to change: you echo `team_id`
per record. Our `KnowledgePageRecord` has no field for it today, so we drop it — but
`cas-f177` will read it if present, and it is the only way a client can ever verify what it
received. Keep echoing it.

### 1.4 What this does and does not fix

Sending `team_id` narrows what *arrives*. It does not add a client-side refusal for what
does arrive — and that gap is real: `pull_knowledge_pages` resolves the project id and then
discards it (`let (url, _project_id) = …`, `syncer/knowledge.rs:228`), and the apply loop
(`:257-273`) never calls `entity_matches_project` (`syncer/pull.rs:73-125`), which every
other entity type runs per row. That is `cas-2cc5`'s work, tracked separately and
deliberately: `cas-f177` is about what we *ask for*, `cas-2cc5` is about what we *accept*.
Both land; neither substitutes for the other. Your `project_id` echo on every record
(§7.3-1) is what makes the second one possible, so thank you for that.

---

## 2. Tombstones — adopt, scoped to knowledge pages, client deletes after the migration

**Our position: adopt your proposal essentially as written.** Tracked as **`cas-e6aa`**.
This is a decision, not a deferral — but the *client* half is sequenced behind the memory
migration, for a reason we explain below rather than leave you guessing at.

### 2.1 What we accept as proposed

- `knowledge_tombstones` as a key in the **existing push envelope**, alongside
  `knowledge_pages`. Reusing the envelope is right: no new route, no new auth path, and it
  inherits gzip and `skipped` reporting.
- Elements of `{ id, deleted_at }`. **`id` alone is sufficient for us** — our local store
  resolves a page by id directly (`KnowledgeStore::get_page`,
  `crates/cas-store/src/knowledge_store.rs:606`, impl at `:1065`), so we do not need
  `rel_path` on the tombstone and would rather not have a second identity on the wire.
- Soft-delete server-side, returned under `knowledge_tombstones` on pull, carried by
  `since` like any other change. Your reasoning is correct: a hard delete is unobservable to
  anyone who has not pulled since.
- **The `locked` guard applies to deletion**, refused cross-account and reported in
  `skipped`, identically to the push guard. This is already true on our side and we want it
  symmetric: a page whose last citing source is tombstoned is hard-deleted locally *unless
  it is locked*, in which case it keeps both its row and its body
  (`knowledge_store.rs:1023-1035`). If a teammate's tombstone could remove a page we
  preserved locally, the lock would mean less over the wire than it does on disk.
- A retention horizon of about a year, chosen for being boring rather than clever. We agree
  with the trade: resurrection on a long-offline machine is a worse failure than storage.

### 2.2 The one addition we would insist on

**Tombstones must carry `project_id` on the pull side, exactly as page records do — and
`team_id` if you have it.** A tombstone is a delete instruction; an unscoped one is strictly
more dangerous than an unscoped page. A foreign page overwrites a same-named local page and
is at least recoverable from its source; a foreign tombstone destroys a page and its body
with nothing left to reconstruct from. Whatever scoping guarantees you give
`knowledge_pages`, tombstones need the same ones, and our per-row refusal (`cas-2cc5`) needs
a field to check.

### 2.3 The honest client-side gap — why this is ours, and why it is post-migration

We cannot emit a tombstone today, and the reason is not the wire format.

**A local page deletion currently leaves no record.** Pages die by provenance cascade inside
`commit_ingest`: when the last citing source is tombstoned, the page row and its FTS row are
deleted and the body file is removed from disk
(`crates/cas-store/src/knowledge_store.rs:992-1035`). The only trace is
`IngestReport.cascade_deleted_page_ids` (`:387-388`) — an in-memory return value that no
caller persists. And our push is watermark-based: it walks live pages and skips anything
with `updated_at <= since` (`syncer/knowledge.rs:160-176`), so a row that has vanished
simply stops appearing. **Absence is not observable on a watermarked push.**

So `cas-e6aa`'s real work is a durable local deleted-pages ledger; serializing
`knowledge_tombstones` is the easy half. That is client-side work we own, and it is why we
are not asking you to build the server half on a promise.

**Sequencing, stated as a gate rather than a date:** `cas-e6aa` is blocked on this document
and sequenced behind the `cas-b129` memory migration settling. During the migration window
the page corpus is actively being written and rewritten by distillation, and the entries/
pages duality is live — shipping a mechanism that makes deletion propagate across machines
while pages are being created, superseded and re-created by migration passes is the worst
possible timing for the one operation that cannot be undone. Spec now, wire it after.

### 2.4 One request about the generic DELETE route

Your `entity_type` allowlist fix ships independently of us — good. When it does, please
**exclude `knowledge_page` from the generic `DELETE /api/sync/{entityType}/{id}` route**
until the tombstone path exists. A `4xx` there is better than the current behaviour, which
by your own analysis is a no-op with extra steps that can be silently reversed by the next
pull. We are not going to call it, but somebody eventually will.

---

## 3. `cas_legacy_id` on the wire — no, this phase

**We are not promoting `cas_legacy_id` to a wire field now, and we are not asking for
server-side dedupe.** Your recommendation to do neither yet is the right one and we are
taking it. We also agree with the boundary underneath it: you should never parse the page
body, and we should never ask you to take a dependency on a hand-rolled client-side
frontmatter format for a convenience feature.

For your reference, the field exists and is stable — `cas_legacy_id` is emitted as a
frontmatter scalar inside the body (`memory_migration/frontmatter.rs:145`, **b129**) — so
promoting it later is a small change on our side. It would still be a client release, and a
release commits us to a correlation contract before we know we want one.

**One correction to how this was framed on our side**, because it does not survive contact
with your §11.3: the reconciliation report you offered *requires* the field. "No field, but
send the report" is not a coherent ask, and we would rather say so than have you discover it
while scoping the work. So:

- **The report is accepted in principle** and is the right first step before anything
  deletes anything. It is gated on the same release as the field.
- **Meanwhile we can answer the same question with zero server work.** During the migration
  window both representations exist locally on every machine — legacy entries in `cas.db`
  and distilled pages in the knowledge store — so "how many pairs exist and how do they
  differ" is a local query for us, not a fleet question. We will run it locally first.
- If local counting shows the duplication matters in a way local data genuinely cannot
  answer, we will come back, promote the field, and take you up on the report in the same
  breath. That is the trigger; there is no other one.

Nothing here blocks you. There is no server work in this item.

---

## 4. The five fixture strings — sequenced, and here is the sequence

**You will get the literal strings. Not yet, and your own second scope note is why.**

Both of your notes are correct. They are `entries`, not knowledge pages, so this is a
pre-existing hygiene item rather than anything the migration introduced. And because the
personal push path re-sends the whole corpus every run, a server-side sweep before the local
cleanup means we both watch the rows come back. That ordering is now recorded as a hard
sequence on our side (`cas-78c8`, GH #156):

1. **Hermetic-test guard lands** — a tripwire at the single choke point where a production
   store is opened, so a test that reaches a real DB panics instead of writing to it, plus a
   suite-level before/after check. The static audit suggested the bleed had already stopped
   (newest fixture row `2026-06-25`); the guard is what proves it stays stopped.
2. **Local purge applies** — `cas purge-test-fixtures`, dry-run by default, matching on
   **exact equality against a pinned five-element constant, never `LIKE`**
   (`memory_migration/routing.rs:57-63`, **b129**; those rows are also deliberately excluded
   from migration by rule R1 at `:182-184`). It takes a backup before applying and verifies
   exact post-condition counts inside the transaction.
3. **Then we send you the five literals**, as a follow-up appended to this file, and you run
   your read-only count → report → we confirm the match set → you delete.

Current local counts, for calibration: **994 rows total — 212 in the global store, 782 in
the `cas-src` project store.** Expect your numbers to differ; ours are two DBs on developer
machines and yours are the fleet's.

We are not asking you to guess a pattern, and we would not accept an inferred match set even
if you offered one. Nothing for you to do on this item until step 3 arrives.

---

## 5. Global / account scope — explicitly deferred, and our current position stated plainly

**Our honest current position: global-scope pages are not synced, by design, in this
phase.** Not blocked, not broken, not an oversight we are about to fix — a deliberate
boundary that we have not yet designed our way past.

The mechanics, so you can see it is a real boundary and not an accident:

- The client **fails closed** without a resolvable project scope on both directions.
  `build_scoped_pull_url` errors with "Cannot pull: not inside a CAS project directory"
  rather than issuing an unscoped pull (`syncer/pull.rs:55-57`), and every push envelope
  requires `get_project_canonical_id()` and errors otherwise (`syncer/push.rs:206-211`,
  `:469-474`).
- The memory migration writes **per root**: global memories become pages in the *global*
  knowledge store, project memories become pages in the *project* store, preserving the
  scope split CAS already has (`memory_migration/mod.rs`, `SourceDb`, **b129**).

So the global corpus exists locally, has no sync identity, and nothing pretends otherwise.
As of today's cutover (§0) that corpus is **39 pages on one machine** — small, real, and
growing. We would rather tell you the number than describe the boundary in the abstract.

**We are neither accepting nor rejecting your `scope: "account"` proposal in this
document.** We do not have the client half designed, and answering now would mean inventing
exactly the kind of semantics we just asked you not to invent for us. What we will say:

- We **agree with your reasoning against a sentinel `__global__` canonical id**, for your
  stated reason. Given four incidents that are all about content crossing a scope boundary,
  a scope with no boundary is the wrong thing to guess at.
- We **agree with `project_id NOT NULL`** and think the audit-then-clone-then-verify path you
  took to get there is the right way to apply a constraint to a live table.
- Your framing — a genuine open design question rather than a bug — is the one we hold too.

**Where it gets designed: `cas-a924`** (knowledge-page sync, client half), currently blocked
on `cas-a41d` and sequenced behind the migration. Worth knowing how that task is written:
its acceptance criteria explicitly permit **loudly gating the gap** instead of closing it —
sync output and docs must state what is excluded, with a per-run count, rather than let a
user believe coverage exists. If we come out of `cas-a924` still without an account-scope
design, the outcome will be a visible, counted exclusion, not silence. Either way you will
hear the result before we ask you for schema.

---

## 6. Capability discovery — accepted in principle, with three semantics we need

**Yes to a `features` array on `/api/me`. No to a separate `/api/capabilities`.** Your
argument wins on its own terms: we already call `/api/me`, it is already authenticated, and
it already carries account-shaped facts we parse and persist — `teams[]` and
`default_team_id` today (`cloud/me.rs:102-111`, written into the user-level cloud config at
`:116`). One more field there costs a read; a second endpoint costs a round trip on every
startup to answer a question that changes about once a year.

This rides **`cas-e000`** and needs a client release. We will schedule it in the same release
as `cas-f177` and `cas-7719` so the fleet pays one upgrade rather than three.

### 6.1 Your second caveat deserves a real answer: here is what the client would *do*

You are right that a `features` list is worthless if the client logs a warning and retries
anyway. It would not.

`KnowledgeEmbedder::from_config` gates on `is_logged_in()` and nothing else, and returning
`None` is already a **first-class supported state** — the comment above it says so:
"`None` means this installation has no semantic channel, which is a first-class supported
state, not an error" (`cloud/embeddings.rs:118-130`). So acting on `features` is not new
machinery. It is one additional condition on a branch that already exists and is already
handled everywhere downstream: no `embeddings` feature → no embedder → pages stay
`pending_embedding`, the sync run reports it, and nothing retries into a void. That is
precisely the failure we described in §11.1, converted from a silent unbounded retry into a
reported state.

### 6.2 Three semantics we need from you in exchange

1. **An absent `features` key means "assume every feature is present."** Not "no features".
   Older servers, self-hosted builds, and any future response trimming must never silently
   disable a working capability. This is deliberately the *opposite* of your `share` rule
   (absent → `private`), because the risk is opposite: wrongly denying a feature breaks a
   working install, while wrongly assuming one degrades to exactly the behaviour we have
   today.
2. **Present-but-empty (`[]`) means genuinely no features** — distinct from absent, and the
   only way to say "this deployment really has nothing".
3. **The tokens are a contract, like `cas-embed-v1`.** Stable, lowercase, additive. We would
   start with `embeddings` and `knowledge_sync`. Please do not repurpose or quietly retire a
   token to signal deprecation — tell us, the same way you committed to renaming rather than
   silently changing the embedding model.

### 6.3 Freshness — deliberately not in the sync path

We would persist the array in the user-level cloud config alongside `teams[]` and
`default_team_id`, written at the existing `/api/me` fetch. That makes it as fresh as the
last login or team fetch, and adds **zero** requests to the sync path. The consequence,
stated so you can object: a newly-enabled feature is picked up at the next `/api/me`, not
instantly. For something that changes about once a year we think that is the correct trade.
If you need instant propagation for some reason we cannot see from here, say so and we will
re-fetch on sync instead.

Your first caveat stands and neither of us can fix it: binaries already in the field cannot
discover anything, and the fleet that would benefit most is exactly the one that cannot use
it. Your `errorClass` instrumentation is the only thing that covers those, which is why it
matters more than the feature flag.

---

## 7. `git_remote` on personal pushes — the joint spec (your §7.3-3)

This is the section your `cas-244f` is waiting on. Our side is **`cas-7719`**, which is
blocked on this spec by design and implements to whatever we agree here. The client field is
purely additive, so you can build against it before we ship it.

### 7.1 Client change (ours, `cas-7719`)

**Field name and placement.** `git_remote`, a top-level string in the personal push
envelope — the same name, the same position and the same value the team push already uses
(`syncer/team_push.rs:45-52`, inserted into the payload at `:222-223`). We are not
introducing a second spelling for a field you already accept.

**Where it goes in.** Personal envelopes are built in two places, both of which already
insert `project_canonical_id` and optionally `team_id`:

- `push_sub_batch` (`syncer/push.rs:451-476`) — the entity path, used by everything
  including knowledge pages;
- `push_sessions` (`syncer/push.rs:190-211`).

`git_remote` goes in beside them, in both. `project_canonical_id` continues to be sent
unchanged and unconditionally — **`git_remote` is additive and never replaces the slug.**

**Value — the shared normalization rule.** In order:

1. `git -C <cas_root> remote get-url origin`
   (`cloud/config.rs:216-231`, `derive_canonical_id_from_git_remote`).
2. Normalize to `<host>/<owner>/<repo>` (`cloud/config.rs:244-279`,
   `normalize_git_remote_url`): strip an `https://`, `http://`, `ssh://git@` or `git@<host>:`
   prefix; strip a `.git` suffix; strip a trailing `/`. Recognized forms are exactly those
   four; anything else yields `None`.
3. **Lowercase.** `.to_lowercase()` at the call site — exactly as `team_push.rs:52` does
   it today.

Step 3 is worth stating explicitly because it is the one place a reader would get this
wrong: **`normalize_git_remote_url` preserves case.** The lowercase is applied by the caller,
not the normalizer, and the code comment at `cloud/config.rs:289-291` records the reason —
your `normalizeGitRemote` lowercases and ours does not, so the comparison in
`canonical_id_from_team_response` is done case-insensitively. `cas-7719` will lowercase at
the call site to match `team_push.rs`, which makes the wire value identical to what your
`resolveCanonicalProject` already normalizes to: strip scheme, `git@host:` form, `.git`,
trailing slash, then lowercase. **Same rule, both sides, one sentence.**

**Absent-remote behaviour: omit the key entirely.** Never `""`, never `null`, never a
filesystem path. All of these produce `None` and must look identical on the wire:

- no `git` binary, or `cas_root` is not a git repo, or there is no `origin` remote
  (`derive_canonical_id_from_git_remote` returns `None`, `config.rs:226-230`);
- a remote URL that is not one of the four recognized forms — local paths included; this is
  pinned by tests at `cloud/config.rs:2036-2044`.

Omitting the key means an old client (which never sends it) and a new client in a non-git
project are indistinguishable to you — which is correct, because they are in the same state:
no remote identity available. It also means "no remote" can never be mistaken for a remote.

**Cost.** At most one extra `git` subprocess per push. The team push already does exactly
this on every push and it has not been a problem; `cas-7719` should reuse the existing
resolution path rather than adding a second spawn.

### 7.2 Server change (yours, `cas-244f`) — our understanding, to confirm or correct

We are writing this as our reading of your §7.3-3, not as instructions. Correct anything we
have wrong before either of us builds.

- Apply on `POST /api/sync/push` the same remote-first resolution `resolveCanonicalProject`
  already applies on the team route: normalized `git_remote` match → alias → `canonical_id`
  match (backfilling `git_remote` when the matched row lacks one) → insert a new project.
- **Only when `git_remote` is present.** Absent → today's behaviour byte for byte: store the
  `project_canonical_id` string verbatim. Every binary in the field must be unaffected, and
  there are a lot of them.
- **No retroactive rewrites.** We agree with your position without reservation: newly
  arriving rows partition correctly, history stays untouched, and no existing bucket gets
  split without a per-case review. A wrong split loses work exactly as badly as a wrong
  merge.
- Personal rows carry `team_id IS NULL`, so the resolution has to key on something like
  `(user_id, git_remote)` rather than `(team_id, git_remote)`. That is the one place your
  team-route logic does not transfer directly. How you index it is yours; we flag it because
  it is the difference visible from here, not because we have an opinion about the index.

### 7.3 The one thing that would break us — please read this before building

**If remote-first resolution ever causes you to echo a `project_id` on pull that differs
from the one the client sent, that client drops 100% of its rows.**

`entity_matches_project` compares the echoed `project_canonical_id`/`project_id` byte-exact
against the locally derived canonical id and rejects every mismatch with a `stderr` warning
and nothing else (`syncer/pull.rs:73-125`, specifically the string comparison at `:106`).
This is the same total-blackout mechanism you measured in §7.3-2b when you evaluated
server-side case folding — you were right to reject it, and the same trap is on this path.

So the hard requirement is: **whatever you store, echo back the `project_id` the caller
asked for on pull.** If remote-first resolution ever implies storing rows under a canonical
id different from the one the client sends, tell us before you build it. That is a
coordinated client release with a migration story, not a server-side implementation detail.

### 7.4 Suggested acceptance

Ours (`cas-7719`): a personal push from a git-backed project carries the lowercased,
normalized remote under `git_remote`; a non-git project (and a project whose remote is a
local path) omits the key entirely; `project_canonical_id` is unchanged in both cases.

Yours (`cas-244f`), as we would recognize it working: two working copies with distinct
remotes that derive the same `project_canonical_id` string land in **distinct** projects; a
push with no `git_remote` behaves exactly as it does today; and a pull for either working
copy echoes back the `project_id` that working copy requested.

### 7.5 Ordering

`cas-244f` does not need to wait for us — the field is additive and accepting it early is
harmless. `cas-7719` starts once you ack this section, so that we build to an agreed shape
rather than to our own reading of it.

---

## What is now unblocked on your board

| Your task | Was waiting on | Answer | Our task |
|---|---|---|---|
| `cas-4b16` | our commitment + wire shape for `team_id` on knowledge pulls | §1 — committed; `team_id=` query param, single active team, four server semantics, absence unchanged | `cas-f177` |
| `cas-244f` | a joint spec for `git_remote` on personal pushes | §7 — field name, normalization rule, omit-when-absent, plus the echo requirement in §7.3 | `cas-7719` |
| `cas-e9e6` (delete-semantics half) | our reaction to the tombstone wire proposal | §2 — adopt as proposed, scoped to knowledge pages, plus `project_id` on tombstones; client deletes post-migration | `cas-e6aa` |

Items 3, 4, 5 and 6 need nothing from you right now: item 3 has no server work at all, item 4
reaches you as a follow-up with literal strings, item 5 comes back to you as a client-side
proposal or a declared exclusion, and item 6 is yours to schedule whenever the `features`
array is convenient.

---

## Files we cited, if you want to read along

| Concern | File (client) |
|---|---|
| Knowledge pull URL, `types=`/`since=`, the discarded project id | `cas-cli/src/cloud/syncer/knowledge.rs:207-286` |
| Knowledge push watermark and `share` derivation | `cas-cli/src/cloud/syncer/knowledge.rs:135-205` |
| Scoped pull URL builder; per-row project refusal | `cas-cli/src/cloud/syncer/pull.rs:38-125` |
| Personal push envelopes | `cas-cli/src/cloud/syncer/push.rs:190-211`, `:451-476` |
| Team push, `git_remote` derivation and placement | `cas-cli/src/cloud/syncer/team_push.rs:45-52`, `:222-223` |
| Canonical-id resolution chain; git-remote normalizer | `cas-cli/src/cloud/config.rs:117-133`, `:216-279` |
| Active-team resolver | `cas-cli/src/cloud/config.rs:940-1000` |
| `/api/me` parse and persistence | `cas-cli/src/cloud/me.rs:83-120` |
| Embeddings capability gate | `cas-cli/src/cloud/embeddings.rs:118-130` |
| Local page store: identity, locked bit, cascade delete | `crates/cas-store/src/knowledge_store.rs:370-388`, `:606`, `:925-945`, `:992-1035` |

Branch-only (**b129**, `epic/memory-knowledge-migration-…-cas-b129`):

| Concern | File |
|---|---|
| Per-root migration destination | `cas-cli/src/memory_migration/mod.rs` (`SourceDb`) |
| `cas_legacy_id` frontmatter emission | `cas-cli/src/memory_migration/frontmatter.rs:145` |
| The five fixture strings; rule R1 | `cas-cli/src/memory_migration/routing.rs:57-63`, `:182-184` |
