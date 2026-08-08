---
task: cas-2cc5
epic: cas-e000
date: 2026-08-08
to: petra-stella-cloud team
type: request (client-side change already shipped; asks for a server-side channel)
status: open
---

# Request: echo the resolved canonical id on the personal push response

## The ask, in one line

The **personal** push response should carry the canonical project id the server
resolved and stored under — the same way the **team** push response already
does — so the client can pin it and detect divergence instead of starving in
silence.

## Why this is being raised

Your `RESPONSE-git-remote-personal-push-ack.md` §3 states that the pull filters
on the `project_id` the client sends and echoes the stored column. We agree with
that design. The consequence, which we want on the record jointly, is:

> If rows are ever stored under a canonical id different from the one the
> client sends, the client does not receive mismatched rows to reject. It
> receives an **empty envelope, indefinitely, with no error on either side.**

That is a failure mode with no error attached. Both sides log a successful
sync; the account simply stops receiving data. It is strictly harder to
diagnose than contamination, because nothing anywhere is wrong-looking.

This is not hypothetical: `resolveCanonicalProject` can legitimately return an
id different from the slug it was given — that is the purpose of its alias and
conflict branches.

## The asymmetry

| Route | Response carries the resolved canonical id? | Client can repin? |
|---|---|---|
| Team push | **Yes** — `canonical_id` / `git_remote` | Yes, and it does |
| Personal push | **No** | **No — nothing to repin from** |

The team route survives id divergence *only* because of that echo. The personal
route has no equivalent channel, so dropping remote-first resolution into the
personal push unchanged would produce exactly the silent-starvation bug above.

## What we have already shipped on our side (cas-2cc5)

We did not build around this silently, and we did not wait for it either:

1. **A starvation detector.** When knowledge pulls come back empty
   persistently *while pushes are being accepted*, the client now emits a loud
   warning naming both the id it pushes as and the id it pulls as. It is
   deliberately advisory — from the client side a divergence is
   indistinguishable from "genuinely nothing new", so it reports rather than
   errors.
2. **Fail-closed ingest** on every pulled row, including knowledge pages, as
   defence in depth if the server-side project filter ever regresses.
3. **Byte-exact canonical-id equality pinned as a protocol invariant**, with a
   test that refuses case, whitespace and trailing-separator variants. We agree
   with your decision *not* to normalize server-side: normalizing would merge
   two distinct projects permanently.

The detector infers the problem from a shape. The echo would let us know it
directly, which is why we are asking.

## What we are asking for, concretely

Add the resolved canonical id to the personal push response body, matching the
team route's existing field name so the client can share one code path:

- On success, return the canonical id the rows were **actually stored under**
  (not merely the one the caller sent).
- If that value differs from what the caller sent, that difference is the whole
  signal — we will pin the returned value and surface the change to the user.

We are **not** asking you to normalize ids, change resolution order, or rewrite
history. Only to tell the caller what happened.

## Evidence class

Everything attributed to the server in this document is **SERVER-ASSERTED** —
taken from your response docs, not reproducible from `cas-src`. The client-side
behaviour described is VERIFIED in this repo. If any of the server-side framing
above is wrong, that correction is more valuable to us than the feature.
