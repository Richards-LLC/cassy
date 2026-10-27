# Commander phone messaging QA, 2026-10-27

Two P1 defects were found on installed v3.82.0. The isolated real hub and
daemon accepted the exercised full-control phone sends once. Live operator
and hosted-model acceptance remains incomplete; this is not product approval.

The complete invariant ledger is **art-32b9e1a3** (task `cas-c6d6`). Fetch it
with `cas artifact fetch --task cas-c6d6 art-32b9e1a3`.

| Finding | Evidence | Follow-up |
| --- | --- | --- |
| A pairing promises message sending, but without `pane:input` it cannot acquire the lease required to send. Retrying cannot repair the permission mismatch. | `art-e11f93b2`, `qa/real-trace.zip` action 3 and `qa/R01-frozen-phone.png` | `cas-d402`, P1 |
| Real operator asks drop choice options. The one-tap question goal is covered by the protocol double but unavailable through the real payload. | `art-e11f93b2`, `qa/adjacent-trace.zip` action 4 and `qa/R07-real-ask-no-choices.png` | `cas-7ebc`, P1 |

The incident investigator ruled the first finding out for the operator's
existing full-scope phone. Tonight's separate causes remain tracked by
`cas-bf20`, `cas-cd73`, `cas-d85c3` and `cas-2109`.

The real isolated walk covered an eight-second CDP lifecycle freeze with
simulated screen lock, Unicode text, rapid double send, a 16,222 UTF-16
message, a queue update while typing, reload during a send, a reply followed
by reload, raw output and keyboard interrupt admission. Read-only database
queries confirmed one row per exercised full-control send. The long payload
equaled the trimmed input; an initial assertion incorrectly expected its
trailing space. Original failures are retained in the evidence.

The disposable provider was inert: queue admission and browser transport are
proven, actual model consumption and response are not. The freeze did not
last an elapsed hour. Busy, ended and recycled hosted-model delivery,
physical Android microphone/keyboard behavior, deployed two-profile Cloud
inbox acceptance, and the live operator send await follow-up. A simple real
reply/reload remained `replied`; this does not refute the specific late-receipt
race of `cas-1b57`.

All 28 tests in the seven assigned journey files passed with Playwright
1.63.0 and four workers. These use protocol doubles and do not establish
daemon or deployed Cloud acceptance. Every assigned invariant and edge path
has a separate real-product verdict in the ledger, including explicit
`NOT EXERCISED` rows.

| Artifact | Contents |
| --- | --- |
| `art-32b9e1a3` | Complete `LEDGER.md`, per-invariant verdicts and deferred proof owners |
| `art-e11f93b2` | Real screenshots, scrubbed traces, ARIA, scripts and queue readbacks |
| `art-7ba189ef` | Native 28-test report, catalog stage captures and ARIA |
| `art-de372050` | Scrubbed full-control real-browser trace |

The backend and frontend under test were
`18a9e881687154a220228ee110d8b385cc98d05e`. Phone and desktop light/dark
captures are in `qa/full-control/real-{390,1280}-{light,dark}.png` inside
`art-e11f93b2`. Strict visual auditing was not executed; no visual pass is
claimed. Design reference: `hub-web/DESIGN.md@ce0d8adc9`.

No live device was paired and no live supervisor was interrupted. Both
isolated devices were revoked and all three owned registry servers stopped.
The artifact-HOME Unix socket length failure is the known, fixed but
unreleased `cas-4927`; a short scratch HOME allowed the isolated walk.
