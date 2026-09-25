---
name: cas-playwright-debug
description: Use when a Playwright test fails, flakes, or times out and you need the root cause and a fix — trace CLI triage, `--debug=cli` stepping, and flake control for Playwright 1.59+.
metadata:
  managed_by: cas
---

# Playwright debugging

Work from evidence in this order: the retained trace, then an interactive
repro, then the fix, then flake proof. Do not edit a test before step 1 names
the failing action.

Check the version first with `npx playwright --version`. The trace CLI and
`--debug=cli` need 1.59+, but `npx playwright cli attach` (the stepping in
section 2) ships only from 1.62; `retryStrategy` needs 1.62+, and test locks and
`locator.visible()` need 1.63+. On an older version, use
`npx playwright show-trace <trace.zip>` and recommend upgrading.

## 1. Read the trace from the terminal

The trace path is printed under the failure, usually
`test-results/<test>/trace.zip`. If no trace was kept, rerun only that test
with `--trace=retain-on-failure`.

```bash
npx playwright trace open test-results/<test>/trace.zip   # metadata + counts
npx playwright trace actions --errors-only                # the failing action id
npx playwright trace action <id>                          # params, call log, source line
npx playwright trace snapshot <id> --phase before         # aria snapshot of the page
npx playwright trace snapshot <id> --phase after
npx playwright trace snapshot <id> -- eval "document.querySelector('#status').textContent"
npx playwright trace requests --failed                    # 4xx/5xx
npx playwright trace console --errors-only
npx playwright trace errors                               # every error with its stack
npx playwright trace close
```

`trace actions --grep <regex>` filters by title. `trace action <id>` shows
which snapshot phases exist. The flag is `--phase`: `before`, `action`, or
`after`. `test-results/<test>/error-context.md` holds the page state at failure.

Classify the failure from the call log before you touch code:

| The trace shows | Cause | Go to |
| --- | --- | --- |
| Locator resolved to 0 elements, or to several (strict mode) | Locator drift | §3 |
| Locator resolved, but the value differs (`"Saved!"` vs `"Saved"`) | The app changed, or the assertion is wrong | Decide which, then §3 |
| An unmocked request failed, or the page lands on sign-in | A route mock missed | §5 |
| The test fails in the suite and passes alone or on retry | A flake | §4 |

When the app is wrong and the test is right, the test stays red. File the bug
instead.

## 2. Reproduce interactively with `--debug=cli`

Start the paused test **in the background**. It waits indefinitely and would
block your pane.

```bash
PLAYWRIGHT_HTML_OPEN=never npx playwright test tests/x.spec.ts:42 --debug=cli > /tmp/pw-debug.log 2>&1 &
# wait until the log prints: Run "playwright-cli attach tw-XXXXXX"
npx playwright cli attach tw-XXXXXX
npx playwright cli -s=tw-XXXXXX step-over            # run the next test call
npx playwright cli -s=tw-XXXXXX pause-at x.spec.ts:57
npx playwright cli -s=tw-XXXXXX snapshot             # aria tree with element refs
npx playwright cli -s=tw-XXXXXX eval "document.title"
npx playwright cli -s=tw-XXXXXX console
npx playwright cli -s=tw-XXXXXX resume
```

- Every command after `attach` needs `-s=<session>`. A bare `step-over`
  targets the `default` session and fails with "browser 'default' is not open".
- Refs like `e4` expire when the page changes. Take a new `snapshot` before
  `generate-locator <ref>` or `click <ref>`.
- Actions you run through the CLI print equivalent Playwright code. Paste that
  code into the test rather than hand-writing locators.
- Stop the background run with `resume` or by killing the process, then rerun
  the single test normally to confirm the fix.

## 3. Write the fix with web-first idioms

| Instead of | Write |
| --- | --- |
| `page.waitForLoadState('networkidle')` | A web-first assertion on the element the next step needs, e.g. `await expect(page.getByRole('table')).toBeVisible()` |
| `page.waitForTimeout(n)` or a sleep | `await expect(locator).toHaveText(...)`, or `await expect.poll(fn).toBe(...)` for non-DOM state |
| `locator('button:visible')` | `page.locator('button').visible()` |
| `frameLocator('#outer').frameLocator('#inner')` when the frame does not matter | `page.frameLocator().getByRole(...)`, which searches every frame |
| `.btn-primary.submit` CSS | `getByRole('button', { name: 'Submit' })`, `getByLabel`, or `getByTestId` |
| `.first()` to silence strict mode | A narrower locator (`{ exact: true }`, `.filter({ hasText })`, or scope to a region) |

Never fix a failure by raising a timeout, adding a sleep, or wrapping tests in
`test.describe.serial`. Each of those hides the cause the trace already showed.

## 4. Prove or kill a flake

```bash
npx playwright test tests/x.spec.ts:42 --repeat-each=20 --workers=4   # repro rate under load
npx playwright test --last-failed                                     # rerun only the failures
```

Make the config separate flakes from real failures:

```ts
export default defineConfig({
  retries: process.env.CI ? 2 : 0,
  retryStrategy: 'isolated',          // retries run last, one at a time
  failOnFlakyTests: !!process.env.CI, // a pass-on-retry still fails CI
  use: {
    trace: {
      mode: 'retain-on-failure-and-retries',
      snapshots: { dom: true, aria: true, screen: true },
    },
  },
});
```

- A test that fails in the suite but passes on an isolated retry is contending
  for something shared. Examples are a seeded account, a DB row, or a settings
  page. Give every test that touches that resource the same lock:
  `test('…', { lock: 'user-settings' }, async ({ page }) => { … })`. Other
  tests stay parallel, including tests in other files and projects.
- A test that also fails on the isolated retry is not a flake. Go back to §1.
- `retain-on-failure-and-retries` keeps a trace for each attempt. Compare the
  first attempt with the retry using `trace actions`.

## 5. Recurring root causes

- **Route mock misses its origin.** The app calls its deployed API origin, and
  the mock pins `localhost`. Match on the path instead:
  `page.route('**/api/users', …)`. Confirm the miss with
  `trace requests --grep api`.
- **Seeded auth arrives late.** `page.addInitScript` must run before
  `page.goto()`. Mock every endpoint the page calls on load. An unmocked 4xx
  often redirects to sign-in.
- **The response shape changed.** Update the mock body, the assertions, and any
  frontend consumer that still reads the old shape.

## Done when

- The single test passes with `--repeat-each=3`.
- The task note records the failing action id, the cause from the trace, and
  the fix.
- The diff adds no sleeps, no `networkidle`, and no raised timeouts.
