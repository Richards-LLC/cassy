---
metadata:
  managed_by: cas
---

# Configuration, targets and reports

## Contents

- [One configuration](#one-configuration)
- [Personas and gated lanes](#personas-and-gated-lanes)
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
  build: { external: ['**/*.mjs'] }, // helper .mjs loads as native ESM
  reporter: [
    ['failures'],
    ['json', { outputFile: 'artifacts/results.json' }],
    ['html', { outputFolder: 'artifacts/report', open: 'never' }],
  ],
  use: { baseURL, headless: true, serviceWorkers: 'block',
    trace: 'retain-on-failure', video: 'retain-on-failure' },
  projects: [
    { name: 'setup', testMatch: 'tests/setup/auth.setup.ts', use: { trace: 'off', video: 'off' } },
    { name: 'hermetic', testMatch: 'tests/hermetic/**/*.spec.ts',
      dependencies: ['setup'], use: { storageState: 'playwright/.auth/qa.json' } },
    { name: 'journeys', testMatch: 'tests/journeys/**/*.spec.ts',
      dependencies: ['setup'], use: { storageState: 'playwright/.auth/qa.json',
        trace: { mode: 'on', snapshots: { dom: true, aria: true, screen: true } },
        video: 'on' } },
  ],
});
```

The shared qa.json example is read-only role state; mutating projects select per-run/worker account state through their fixture. Select hermetic on PRs. Default fixtures are test-scoped; setup writes state with real sign-in, not a page shared across tests. Ignore auth files in Git and restrict artifact access.

Specs that import helper `.mjs` need `build.external`: on Node with `module.registerHooks` (22.15+), Playwright compiles each required `.mjs` as CommonJS, so a top-level `import.meta` throws and the file loads zero tests. Externals load through native `require(esm)`, which accepts synchronous ESM only (no top-level await). Fix the config rather than pinning an older Node. Guard it with `playwright test --list` on the current Node: exit 0 and a non-zero test count.

## Personas and gated lanes

Generate one project per signed-in persona from a list that the setup project also reads, so the two cannot drift. Route specs by a title tag and gate costly or target-bound tags with environment variables:

```ts
// tests/config/personas.ts, imported by auth.setup.ts and the config
export const PERSONAS = ['creator', 'admin'] as const;
export const statePath = (p: string) => `playwright/.auth/${p}.json`;

// playwright.config.ts
const gated = [
  ...(process.env.E2E_LIVE === '1' ? [] : [/@live\b/]),          // spends real money
  ...(process.env.TEST_ENV === 'staging' ? [] : [/@staging\b/]), // needs deployed staging
];
projects: [
  // setup as above; every other existing project adds grepInvert: [...gated, /@as-/]
  ...PERSONAS.map((p) => ({ name: `as-${p}`, testMatch: 'tests/hermetic/**/*.spec.ts',
    dependencies: ['setup'], grep: new RegExp(`@as-${p}\\b`), grepInvert: gated,
    use: { storageState: statePath(p) } })),
],

// spec: the page is already signed in as creator
test('creator publishes a draft @as-creator', async ({ page }) => { /* … */ });
```

- Each test gets a fresh signed-in context, and the report names the persona (`[as-admin] › …`). A missing persona secret fails setup; a missing state file fails that project.
- Write tags in the title string, not the `{ tag: [...] }` option. Playwright appends option tags to the text `--grep` matches, so Cassy's changed-test stability run, which selects one test with an anchored title grep, finds zero tests and the delivery cannot park (GH #2349, open).
- Project-level `grepInvert` and `testIgnore` replace the top-level ones, so spread the shared lists into every project.
- The environment variable is the only switch. Run a paid lane with `E2E_LIVE=1 npx playwright test --grep @live` and post-deploy smoke with `TEST_ENV=staging npx playwright test --grep @staging`. A command-line `--grep-invert @live` is opt-out, so a whole-package or agent run that forgets it spends money.
- Prove the gate with `--list` with and without the variable: zero tests, then N.

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

Attach final.aria.json from await page.locator('body').ariaSnapshotJSON() in test-scoped teardown on failures and on successful journeys, whenever the page remains available. Retain the trace and video on success for journeys; an existing retained screencast can satisfy video without duplicating frames. Use the established evidence wrapper for captions, redaction and capture manifests. Never label a DOM-only trace as an ARIA capture.

Take screenshots at the fresh page's actual viewport. A time-logic test installs page.clock before the app reads time; normal IO uses events and web-first expectations. Full integration/journey evidence remains with the project's existing landing/batch owner.

Sources: [config](https://playwright.dev/docs/test-configuration), [authentication](https://playwright.dev/docs/auth), [network](https://playwright.dev/docs/network), [reporters](https://playwright.dev/docs/test-reporters).
