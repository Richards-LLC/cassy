---
metadata:
  managed_by: cas
---

# Configuration, targets and reports

## Contents

- [One configuration](#one-configuration)
- [Network and target policy](#network-and-target-policy)
- [Honest reports](#honest-reports)
- [Evidence fixtures](#evidence-fixtures)

## One configuration

Adapt this shape inside the existing canonical config; do not add a second gate/config per task. The origins are examples of a disposable local fixture; substitute reviewed local/staging origins explicitly, never a production host.

```ts
import { defineConfig } from '@playwright/test';

const baseURL = process.env.E2E_BASE_URL ?? 'http://127.0.0.1:4173';
const apiURL = process.env.E2E_API_URL ?? 'http://127.0.0.1:4180';
const allowed = new Set(['http://127.0.0.1:4173', 'http://127.0.0.1:4180']);
for (const url of [baseURL, apiURL]) {
  if (!allowed.has(new URL(url).origin)) throw new Error('Unapproved test origin');
}
export default defineConfig({
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  failOnFlakyTests: true,
  reporter: [
    ['failures'],
    ['json', { outputFile: 'artifacts/results.json' }],
    ['html', { outputFolder: 'artifacts/report', open: 'never' }],
  ],
  use: { baseURL, headless: true, serviceWorkers: 'block',
    trace: 'retain-on-failure', video: 'retain-on-failure' },
  projects: [
    { name: 'setup', testMatch: /auth\.setup\.ts/ },
    { name: 'hermetic', testMatch: 'tests/hermetic/**/*.spec.ts',
      dependencies: ['setup'], use: { storageState: 'playwright/.auth/qa.json' } },
    { name: 'journeys', testMatch: 'tests/journeys/**/*.spec.ts',
      dependencies: ['setup'], use: { storageState: 'playwright/.auth/qa.json',
        trace: { mode: 'on', snapshots: { dom: true, aria: true, screen: true } },
        video: 'on' } },
    { name: 'smoke', testMatch: 'tests/smoke/**/*.spec.ts',
      dependencies: ['setup'], use: { storageState: 'playwright/.auth/qa.json' } },
  ],
});
```

Select hermetic on PRs; select live smoke explicitly after a reviewed staging deploy. Default fixtures are test-scoped; setup writes state with real sign-in, not a page shared across tests. Add a project per role where required. Ignore auth files in Git and restrict artifact access.

## Network and target policy

Validate both browser and API clients, including redirects and setup/cleanup endpoints; an approved starting URL does not authorize a redirected destination. Use an exact-origin allowlist and fail on attempts outside it. Check mutation authorization independently of navigation permission, before issuing writes or cleanup.

In hermetic fixtures, install context routing before creating/navigating pages. Fulfill only reviewed fixture endpoints; allow only the fixture app/assets and explicitly abort/log every other request. Verify the final external-request count is zero. Context routing covers popups; block Service Workers when interception cannot observe them. Route WebSockets with the installed API or deny them at the fixture/server boundary; document other network channels. A wildcard API path without an origin check can leak to a deployed service.

Give each run/shard unique ports and data namespaces. A Playwright lock has no cross-invocation guarantee; use an external lease only when isolation cannot remove shared state.

## Honest reports

Integrate these checks into the existing command/report consumer, after merging shard reports:

- Fail globally zero collected/executed required tests; an individually empty shard is valid if the aggregate has required coverage. Banned workaround: --pass-with-no-tests for an empty required suite.
- Fail missing required auth, secrets or fixture data before execution; inspect workflow conditions to prove the required lane ran. A green workflow whose test job never ran is missing coverage.
- Declare optional lanes in the reviewed project policy, emit each skip's reason in JSON/JUnit/HTML and summarize them. Fail unexplained skips. A comment alone does not enforce anything: parse the full report's results/annotations and compare skipped ids to the declared optional inventory.
- Execute known product defects with test.fail(condition, reason) and a tracked owner; an unexpected pass fails. Conditional boolean/callback skip/fixme/fail/slow overloads remain valid in 1.65; the description-only overload is additive. Policy still forbids hiding required product failures with skips.
- retries: 0 prevents masking; failOnFlakyTests remains true if a diagnostic override is used. Preserve the original failed result. Assertion polling is expected synchronization.

Keep full journey reports, including success captures. onlyFailures is permitted for non-journey HTML/JSON/JUnit only when no pass-evidence obligation exists. Use the failures reporter for concise console output without deleting structured successes.

## Evidence fixtures

Attach final.aria.json from await page.locator('body').ariaSnapshotJSON() in the journey teardown, including failed attempts when the page remains available. Retain the trace and video on success for journeys; an existing retained screencast can satisfy video without duplicating frames. Use the established evidence wrapper for captions, redaction and capture manifests. Never label a DOM-only trace as an ARIA capture.

Take screenshots at the fresh page's actual viewport. A time-logic test installs page.clock before the app reads time; normal IO uses events and web-first expectations. Full integration/journey evidence remains with the project's existing landing/batch owner.

Sources: [config](https://playwright.dev/docs/test-configuration), [authentication](https://playwright.dev/docs/auth), [network](https://playwright.dev/docs/network), [reporters](https://playwright.dev/docs/test-reporters).
