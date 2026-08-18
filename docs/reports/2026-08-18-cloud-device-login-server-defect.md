# Server defect: `/device` approval page rejects a valid code with "Missing or invalid Authorization header"

**Status:** open, **not fixable in this repository**. The CLI half is fixed (cas-046d);
this document exists so the server half is escalated rather than silently dropped.

**Component:** Petra Stella Cloud web app — the device-authorization page at
`https://petra-stella-cloud.vercel.app/device` and the endpoint it posts to.
That code lives in the cloud/Next.js project, not in `cas-src`.

**Reported by:** Ben, macOS clean install, 2026-08-18 (field report finding #2).

## Symptom

With an active, signed-in browser session on the cloud app, opening the device
page with a freshly issued code:

```
https://petra-stella-cloud.vercel.app/device?code=FEUE-NMWQ
```

fails with:

```
Missing or invalid Authorization header
```

The approval never completes, so the waiting `cas login` eventually times out.

## Reproduction

1. On a machine with the Cassy CLI, run `cas login`.
2. Note the `user_code` printed by the CLI (for example `FEUE-NMWQ`).
3. In a browser already signed in to `https://petra-stella-cloud.vercel.app`,
   open `https://petra-stella-cloud.vercel.app/device?code=<user_code>`.
4. Observe the error text above instead of an approve/deny prompt.

Reproduced with three separate codes in one session. The session was signed in
throughout — a page reload of the dashboard in the same browser worked.

## Scope: what is and is not the CLI

Two defects were interleaved in the original report; only the first was ours.

- **CLI defect (fixed here, cas-046d).** `cas login` built the browser URL by
  appending `?code=<code>` to a `verification_uri` that already carried the
  code, producing `…/device?code=FEUE-NMWQ?code=FEUE-NMWQ`. The page then read
  the whole `FEUE-NMWQ?code=FEUE-NMWQ` blob as the code. Fixed in
  `cas-cli/src/cli/auth.rs` (`build_verification_url`), with regression tests.
- **Server defect (this document).** After hand-correcting the URL to a single
  `?code=<code>`, the page still fails with "Missing or invalid Authorization
  header". No CLI change can affect that: at this point the CLI is only polling
  `POST /device/token`, and the browser is talking to the web app on its own
  session cookie.

## Likely shape of the server bug

The message is an API-side auth rejection, which suggests the device page is
calling its own approval API with a bearer-token expectation (`Authorization:
Bearer …`) while the browser only has a session cookie — i.e. the page never
attaches credentials, or attaches the wrong kind. Worth checking on the server:

- the fetch the `/device` page makes when it loads or when the code is
  submitted, and whether it forwards the session;
- whether the approval route requires `Authorization` rather than accepting the
  authenticated session;
- whether the route is running on an edge/middleware path that strips cookies.

## Mitigation shipped in the CLI

Until the page is fixed, the device flow can fail through no fault of the CLI,
so every device-flow screen (and the fatal-error path) now names the working
alternative:

```
cas login --token <API-TOKEN>
```

That path does not touch the `/device` page at all: it verifies the token
against `/api/sync/status` and stores it in `~/.cas/cloud.json`. It works from
any directory and logs in every project on the machine.

## Escalation

Owner: the cloud web app maintainer. This file is the Cassy-side record; the CLI
work is complete and the remaining fix is a server change.
