# Live pairing verification

Status: PASS
Run date: 2026-09-07
Assembled source: `698dbabae6e4cdf6e774249e12268bc9c80031a8` (hub-web unchanged by the later epic fold)
Hub: `cas 3.17.3 (698dbab-dirty 2026-09-07)`; the dirty marker is from the temporary scratch `hub-web/dist` embedding used for this verification build, not a source edit.
Browser: Google Chrome for Testing 153.0.8010.12 (Chromium 1243 Playwright executable), headless persistent fresh profile per scheme/scenario.
Hub origin: `http://127.0.0.1:4317` (isolated scratch `HOME` and `CAS_ROOT`; host `~/.cas/hub` was not read or written).

## Happy path

The local Commander was built from the assembled source into a scratch Vite outDir, embedded into a release-fast `cas` binary, and served by a registered local hub. Read-only copies of the active session metadata populated the fleet catalog, so the final frame exercised the real catalog rather than an empty fixture.

Each row has four captures: light/dark at 1280×800 and 390×844.

| Step | Result | Captures | Live-region / visible evidence |
| --- | --- | --- | --- |
| Address guidance, one primary action | PASS | [`happy-address-guidance-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `Create pairing code`; exact origin and scope summary are visible. |
| Invitation entered | PASS | [`happy-invitation-entered-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `One-time invitation ready. Confirm the target hub.`; URL, machine label, scope ceiling, device and operator fields are present. |
| Exchange | PASS | [`happy-exchange-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `Creating this browser credential… Cancel stops local installation.` and `Pairing…` are visible while the real exchange is held in flight. |
| Saved access | PASS | [`happy-saved-access-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `Access saved — connecting to Unit 8 live hub…` is visible before catalog resolution. |
| First connection / populated fleet | PASS | [`happy-connected-fleet-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `Unit 8 live hub connected`; the fleet hero shows 3 sessions (2 idle, 1 stale) and the session ledger. |

## Wrong-code / retry path

The assembled Commander has no free-form pairing-code input: `cas hub pair` supplies a one-time invitation URL. The supported recoverable wrong-entry branch is an incorrect machine hub address. I entered `http://127.0.0.1:4318`, observed the failed exchange copy while the invitation remained owned by the dialog, then corrected it to the live hub origin and retried the same invitation.

| Step | Result | Captures | Evidence |
| --- | --- | --- | --- |
| Failed exchange with retry owner | PASS | [`wrong-code-retry-failed-exchange-retry-owner-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `We could not confirm pairing… tap Pair again`; the form remains open and the pending invitation is recoverable. |
| Retry succeeds | PASS | [`wrong-code-retry-retry-succeeds-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | Corrected origin installs access and reaches the populated fleet; `Unit 8 retry hub connected`. |

## Cancel path

The exchange POST was held after the local hub accepted it. Cancel was pressed while the browser showed `Creating this browser credential…`; the browser showed `Pairing cancelled.` and did not retain access. The hub-side credential created before the cancellation response was revoked during cleanup.

| Step | Result | Captures | Live-region / visible evidence |
| --- | --- | --- | --- |
| Cancel during exchange | PASS | [`cancel-exchange-in-flight-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `Cancel stops local installation.` and `Cancel Pairing…` are visible. |
| Cleanup and completion | PASS | [`cancel-cancel-cleanup-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) and [`cancel-cancel-complete-{light,dark}-{1280,390}.png`](captures/after/pairing-live/) | `Pairing cancelled.`; no machine remains in the browser catalog. |

## Visual QA and cleanup receipts

- `npm run visual-qa` in `hub-web`: **PASS 9 fixtures × 2 schemes × 2 viewports**.
- `node scripts/visual-qa.mjs --strict --allowlist hub-web/visual-qa-allowlist.json` against the live pairing URL: **PASS**.
- The same strict runner against both the live hub root and live pairing URL: **PASS** for light/dark and 1280/390.
- Five scratch credentials were created across the happy, retry, and cancel runs. `cas hub auth list --json` after cleanup: `total=5`, `active=0`, `revoked=5`.
- The registered local hub on port 4317 was stopped before delivery. No host hub process or host `~/.cas/hub` state was modified.
- No hub-web source or tracked `dist` files were changed. No defects were observed, so there is no owning-unit send-back.
