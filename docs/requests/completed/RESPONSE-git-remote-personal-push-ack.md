> **Disposition (2026-08-07, cas-ab75):** Reply, not a report — archived per `docs/requests/README.md`. Acknowledgement from the cloud team on the `git_remote` personal-push spec; the open client-side decision it names is tracked as CAS task `cas-7719`, not as a staged file.

# ACK — `git_remote` on personal pushes (your §7)

**Status:** Delivered 2026-08-07; signed off by cloud supervisor.
**Responds to:** `RESPONSE-cloud-open-decisions-and-git-remote-spec.md` §7
**Our task:** `cas-244f` (server half) · **Yours:** `cas-7719` (client half)
**Bottom line:** §7.1 accepted as written, no objections. §7.2 is a correct reading of our
position. §7.3 accepted as a hard constraint without qualification. **§7.4 contains a
contradiction we cannot implement as stated** — it is named in full below with three
resolutions and a recommendation. Per your §7.5, `cas-7719` should not start until you have
picked one, because the choice changes what your client sends.

---

## 1. §7.1 — the client change: accepted as written

No corrections. Specifically we accept, and will build against:

- **`git_remote`, top-level string, personal push envelope** — same name, same position,
  same value as the team push already uses. Agreed that a second spelling for a field we
  already accept would be the wrong call.
- **Both envelope builders** (`push_sub_batch`, `push_sessions`), beside
  `project_canonical_id` and `team_id`.
- **`project_canonical_id` continues unconditionally.** We are relying on this. It is the
  key our storage and pull filter are built on, and §3 below explains why that matters more
  than it might look.
- **The normalization rule, including step 3.** Your reading of the split is right and worth
  us confirming from our side: our `normalizeGitRemote` (`lib/projects.ts:32-43`) strips
  `https://` / `http://` / `git://`, rewrites `git@<host>:` to `<host>/`, strips a `.git`
  suffix and a trailing slash, and **lowercases the whole string** as its last step. So a
  value your caller lowercases at the call site (matching `team_push.rs:52`) is byte-identical
  to what we normalize to. Same rule, both sides — confirmed, not just agreed.
  - One difference worth stating so nobody trips on it later: we do **not** recognize
    `ssh://git@` as a distinct form; it is handled because `ssh://` is not stripped by our
    scheme regex, so `ssh://git@github.com/o/r` normalizes to `ssh://git@github.com/o/r`
    rather than `github.com/o/r`. If your step-2 list genuinely emits that form, tell us and
    we will add it to the regex. If, as we read `cloud/config.rs:244-279`, the four
    recognized forms all collapse to `<host>/<owner>/<repo>` **before** they reach the wire,
    then this never matters and no change is needed. Please confirm which.
- **Absent → omit the key entirely.** Never `""`, never `null`, never a filesystem path. We
  will treat all three of those as "absent" defensively rather than trusting it, but we agree
  the contract is omission, and we agree an old client and a new client in a non-git project
  should be indistinguishable to us, because they are in the same state.

---

## 2. §7.2 — your reading of our position: correct as written

All four bullets are right. Confirming each with the code behind it, so you are not taking
our word for it:

1. **Same remote-first resolution as the team route.** That is
   `resolveCanonicalProject` (`lib/projects.ts:70`), and its order is exactly as you
   describe: normalized `git_remote` match → alias match → `canonical_id` match (backfilling
   `git_remote` onto the matched row when that row has none) → insert.
2. **Only when `git_remote` is present; absent is byte-for-byte today.** Agreed, and it is
   the same posture we shipped for `team_id` on the knowledge pull last week. We will not
   change a single code path for a request that omits the key.
3. **No retroactive rewrites.** Agreed without reservation, and thank you for stating the
   symmetry — a wrong split does lose work exactly as badly as a wrong merge.
4. **Personal rows key per-user, not per-team.** Correct, and it is the one real porting
   problem. Our `projects` table and its partial-unique indexes are keyed on `team_id`; every
   query in `resolveCanonicalProject` filters `WHERE team_id = ...`. Personal rows carry
   `team_id IS NULL`, so a personal resolution needs a per-user equivalent
   (`(user_id, git_remote)`) and its own uniqueness guarantee. That is ours to solve and we
   are not asking you for anything here — flagging only that it is schema work, not a
   parameter change, which is part of why we want §7.4 settled before we cut it.

---

## 3. §7.3 — accepted as a hard constraint, and it bites us harder than you described

Accepted without qualification: **we will echo back the `project_id` the caller asked for.**

One correction to the failure model, in your favour. You describe the client-side symptom —
`entity_matches_project` compares byte-exact and rejects every mismatch with a `stderr`
warning (`pull.rs:73-125`, comparison at `:106`). On our side the failure would usually not
even reach that check. Our pull **filters** on the requested value
(`eq(syncEntities.projectId, projectId)`, `app/api/sync/pull/route.ts`) and echoes the stored
column back. So if we ever stored rows under a canonical id different from the one the client
sends, the client would not get mismatched rows to reject — it would get **an empty
envelope**, indefinitely, with no warning on either side. Silent starvation, not a loud drop.
That is worse to diagnose than what you described, and it is why we are treating §7.3 as
load-bearing rather than as a caution.

**This is not hypothetical for the code as it stands.** `resolveCanonicalProject` can and
does return a canonical id different from the slug it was given — that is the entire point of
steps 1, 2 and the step-4 conflict path. On the team route we get away with it because the
push **response** carries `canonical_id` and `git_remote` back
(`app/api/teams/[teamId]/sync/push/route.ts:218`), and your client repins from it
(`canonical_id_from_team_response`, which you cite in §7.1). **The personal path has no such
channel today** — our personal push response does not return a canonical id, and your
personal path has nothing to repin from. So dropping `resolveCanonicalProject` into the
personal push unchanged would be precisely the silent-starvation bug above. We will not do
that, and §4 is the direct consequence.

---

## 4. §7.4 — the contradiction

Your two acceptance bullets for `cas-244f` cannot both be satisfied by the wire that §7.1
specifies.

> (a) "two working copies with distinct remotes that derive the same `project_canonical_id`
> string land in **distinct** projects"
> (b) "a pull for either working copy echoes back the `project_id` that working copy
> requested"

Take the case (a) is actually about: **one user**, two working copies, distinct remotes,
which by construction derive the **same** `project_canonical_id` string — say `accounting`.
Push carries `git_remote`, so we can tell them apart and (a) is achievable on the push path.

**Pull carries no such field.** It is a GET with `project_id` and nothing else. Both working
copies send `project_id=accounting`, from the same account, with the same bearer token. At
pull time the two are **byte-identical requests**. If we honoured (a) and stored them as
distinct projects, we would then have to answer that request, and every option violates
something:

- **Return the union** — the split is undone at the only moment it matters. Worse, this is
  precisely the cross-project contamination we eliminated in `cas-2eb3` and would be a
  regression of a shipped fix.
- **Pick one** — one working copy silently receives the other's rows, or receives nothing.
  Non-deterministic data loss.
- **Return rows stamped with a server-derived distinct id** — violates (b) and §7.3, and per
  §3 above manifests as an empty envelope forever.

So (a) is a property of the **push** path, (b) is a property of the **pull** path, and the
pull path has no disambiguator. The contradiction is not in your reasoning; it is that
`git_remote` was specified as push-only while the acceptance criterion needs it on both.

**A second, sharper form of the same tension**, since it constrains the fix: §7.2's alias and
`canonical_id` branches can resolve to a project whose canonical id differs from the string
sent. Under §7.3 we may never let that resolved id become the value we store-and-echo. Which
means remote-first resolution on the personal path can only ever be **bookkeeping** — it can
record identity and backfill `git_remote`, but it cannot change what a pull is keyed on.
Bookkeeping-only resolution is, by definition, not observable as "distinct projects". §7.4
asks for the one thing §7.3 forbids.

### Resolutions

**Option A — bookkeeping-only (ship now, no client coupling).**
We accept `git_remote` on the personal push, normalize it, and use it to backfill and to
record remote identity. We store and echo the client's `project_canonical_id` verbatim,
always. §7.3 is satisfied absolutely; nothing in the field changes behaviour; `cas-7719` ships
whenever you like with no ordering constraint. **Cost:** §7.4(a) is not delivered — two
same-slug working copies stay in one bucket until one of the options below lands. We would be
building the accepting half of the protocol and the identity record, not the split.

> **To be explicit, because it changes your scheduling: under Option A we accept and
> *persist* `git_remote` from day one.** It is not parsed-and-discarded and it is not
> deferred to the follow-up. Every personal push that carries a remote has that remote
> recorded against the project from the moment this ships. So by the time Option B lands the
> identity data is **already in place and already accurate** — B becomes a pull-side filter
> over data we have been accumulating, not a backfill campaign, and there is no window where
> your clients are sending a field into a void. **`cas-7719` does not need to wait on our
> follow-up, or on our decision about it.** Start sending the field whenever you ship; the
> worst case is that we are recording it and not yet splitting on it.

**Option B — `git_remote` on the pull too.**
Your client sends the same normalized, lowercased value as a query parameter on
`GET /api/sync/pull` (omitted when absent, exactly as on push). We resolve identically on both
paths and still echo the caller's `project_id` string verbatim. This is the only option that
actually delivers §7.4(a) **and** §7.3 together, and it needs no migration: the echoed value
never changes, only the filter narrows. **Cost:** one more call site in `cas-7719`, and a
rollout order — the server must accept-and-ignore the param before your client sends it,
which is free (we did exactly this for `team_id` on the knowledge pull).

**Option C — server-derived distinct stored ids + client repin.**
We store the second working copy under a distinct canonical id and hand it back so the client
repins — the mechanism your team path already uses via `canonical_id_from_team_response`. It
is proven on both sides, which makes it more tempting than it should be. But on the personal
path it changes the value a client is pinned to, for data it has already synced, which is the
"coordinated client release with a migration story" your own §7.3 says must not be a
server-side implementation detail. We raise it to show it was considered and rejected on your
reasoning, not ours. If you think the team-side repin path generalizes more cheaply than we
believe, say so — you know that code and we do not.

**Option D — narrow §7.4(a) to cross-user only.**
Distinct-projects semantics apply only where two working copies belong to **different**
accounts, which per-user scoping already isolates, and the single-user same-slug case is
declared out of scope. This is honest about what is achievable without a wire change, but it
delivers nothing new: cross-user isolation is already how personal rows behave today
(`user_id` is in the primary key). It is really Option A with §7.4 reworded to match.

### Our recommendation

**Option A now, Option B as the follow-up that unlocks §7.4(a) — and B is your call to make.**

Reasoning: A is the minimal change that satisfies §7.3, and §7.3 is the constraint where being
wrong is unrecoverable and silent. A is shippable immediately, is additive, has zero coupling
to your release, and leaves every field binary untouched. It also gets the accepting half of
the protocol into production so that when B lands, the only new work is the pull filter.

We recommend B rather than C or D for the split itself because B is the only option that adds
a disambiguator where the ambiguity actually is, and it does so without ever changing a value
a client is pinned to — no migration, no repin, no blackout risk. But **it is your protocol
and your call**: B costs you a call site in `cas-7719` and the single-user same-slug case may
simply not be worth it to you. If you would rather stay at A indefinitely, that is a coherent
choice and we will not push for B; we would just ask you to reword §7.4(a) so the acceptance
criterion matches the wire, per Option D.

Tell us which and we will build to it. Until then `cas-244f` ships Option A only.

---

## 5. What we are committing to, concretely

Assuming Option A (revise if you pick otherwise):

- `POST /api/sync/push` accepts **and persists** an optional top-level `git_remote`;
  normalized with the same rule as the team route; `""` / `null` / unrecognized-form all
  treated as absent. Recording starts the day this ships, not the day the split does.
- Absent → today's code path, byte for byte.
- Remote identity recorded per-user (`(user_id, git_remote)`), backfilled onto existing rows
  that lack one; **no retroactive rewrites**, no bucket splits, no existing row's
  `project_id` changed.
- `project_canonical_id` stored and echoed **verbatim, always** — this is the invariant we
  will write the tests against, not a best effort.
- Acceptance we will demonstrate: a personal push with `git_remote` records the remote and
  changes nothing else observable; a push without it is indistinguishable from today; a pull
  from either working copy returns that working copy's rows stamped with the `project_id` it
  asked for.

## 6. Ordering

Per your §7.5: `cas-7719` does not need to wait on us for Option A — the field is additive, we
will accept it before you send it, and per §4 we persist it from day one, so nothing you send
is wasted while the §7.4 question is open. If you choose Option B, the only ordering
constraint is that our pull-side change ships before your client starts sending the
parameter, and we will tell you when it is live.

---

## 7. §7.2 posture — the confirmation you asked for, in one place

You wrote §7.2 as your reading of our position and asked us to confirm or correct it before
either side builds. Confirming it as a block, so there is a single paragraph to hold us to:

1. **Your reading is correct as written.** We found nothing to correct in §7.2. The
   resolution order, the only-when-present condition, the no-rewrites stance and the
   per-user-keying observation are all accurate statements of what we intend to build.
2. **Resolution is keyed per-user for personal rows.** Personal rows carry `team_id IS NULL`,
   so the personal path keys on `(user_id, git_remote)`, not `(team_id, git_remote)`. Your
   flag was right and it is schema work on our side, not a parameter change.
3. **No retroactive rewrites. Ever.** No existing row's `project_id` is changed, no existing
   bucket is split or merged, and no history is rewritten by this work. Newly arriving rows
   partition correctly and everything already stored stays exactly where it is. If we ever
   believe a specific bucket needs splitting, that is a per-case review with you, not a
   migration we run.
4. **An absent `git_remote` means today's behaviour, byte for byte.** Not "mostly the same" —
   the same code path, the same stored value, the same response. Every binary in the field is
   unaffected, and `""`, `null` and an unrecognized form are all treated as absent rather than
   trusted.
5. **`project_canonical_id` is stored and echoed verbatim, always.** This is the invariant we
   will write the regression tests against, and it is the one thing in this document we are
   treating as non-negotiable regardless of which §7.4 option you pick.
