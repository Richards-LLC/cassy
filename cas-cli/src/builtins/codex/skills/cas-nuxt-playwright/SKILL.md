---
name: cas-nuxt-playwright
description: Use when writing or debugging Playwright E2E tests for a Nuxt 3 or Nuxt 4 app with Firebase auth and Quasar UI — SSR-mode detection, auth-state reuse, selector and timing triage.
metadata:
  managed_by: cas
disable-model-invocation: true
---

# Nuxt + Playwright E2E Testing

Guide for writing and debugging Playwright E2E tests against Nuxt (3 & 4) apps with Firebase auth and Quasar UI. Grounded in real failures across production projects. Targets `@playwright/test` **1.63**. Check the installed version with `npx playwright --version`. If it is older, upgrade before using the APIs marked with a version below.

## Step 1: Detect the SSR mode

Read `nuxt.config.ts` before writing any test. Everything else follows from this.

### Global SSR off (`ssr: false`)

- `page.goto()` works everywhere — SPA shell loads, client-side middleware runs
- Auth tokens must be in localStorage before navigation
- No server-side middleware exists — all middleware runs in the browser

### Per-route SSR via `routeRules`

- Public pages (`ssr: true`) render server-side — `page.goto()` is safe
- Protected pages (`ssr: false`) run client-side middleware
- **SSR middleware cannot see Firebase IndexedDB tokens.** `page.goto('/protected')` on an SSR-enabled route will redirect to sign-in even with valid browser auth
- Use the `navigateTo()` helper (section below) for protected routes on SSR apps

### Decision tree

1. `ssr: false` globally? → SPA. `page.goto()` works everywhere with auth in localStorage.
2. Target page is `ssr: false` in `routeRules`? → SPA page. `page.goto()` works with auth.
3. Target page is `ssr: true` and public? → `page.goto()` works (no auth needed).
4. Target page is `ssr: true` and protected? → **Use `navigateTo()`.** SSR middleware will redirect.

## Step 2: Choose the auth pattern

### Pattern A: Real login + worker-scoped fixture (recommended default)

One login per Playwright worker, shared across all tests in that worker. Tests hit the real app.

See `references/auth-fixture-template.md` for the ready-to-copy fixture and the 1.63 `playwright.config.ts`.

**When to use:** Default choice for testing against staging or production. The fixture logs in once via the real sign-in form, then every test in the worker reuses that authenticated page.

**Key properties:**
- Worker-scoped (`{ scope: 'worker' }`) — login happens once, not per test
- Includes `navigateTo()` helper for protected routes on SSR apps
- Includes cleanup tracker for test-created resources
- Multi-environment support via `getEnvConfig()` helper
- Tests that mutate the shared account declare a `lock` (see "Flake control")

### Pattern B: storageState from Firebase REST API

A setup project signs in via the Firebase REST API, seeds localStorage, and saves storageState for dependent test suites. Use the Web Storage API (`page.localStorage`, 1.61+). It acts on the current origin, so load a public page first.

```ts
// In a setup file (runs before test suites)
const auth = await signInWithFirebaseREST(email, password);

await page.goto('/'); // any public page on the app origin
// Seed both Firebase SDK key AND Pinia auth store
await page.localStorage.setItem(`firebase:authUser:${apiKey}:[DEFAULT]`, JSON.stringify(fbUser));
await page.localStorage.setItem('your-app-auth', JSON.stringify(appAuth));

await page.context().storageState({ path: statePath });
```

**When to use:** Setup projects that pre-authenticate for dependent suites. Faster than UI login but couples to Firebase REST API shape.

**Critical:** When the app writes the Pinia store itself, for example after a UI login, wait for persistence before saving storageState:
```ts
await expect
  .poll(() => page.localStorage.getItem('your-app-auth'), { timeout: 10_000 })
  .not.toBeNull();
```

### Pattern C: addInitScript + full route mocking

Fully isolated tests with no backend dependency. `page.localStorage` needs a loaded origin. Auth must exist *before* the first app script runs, so this pattern keeps `addInitScript`.

```ts
// Seed auth BEFORE page.goto()
await page.addInitScript((val: string) => {
  localStorage.setItem('your-app-auth', val);
  // Clear Firebase keys — prevents SDK from calling getIdToken()/reload()
  for (const key of Object.keys(localStorage)) {
    if (key.startsWith('firebase:')) localStorage.removeItem(key);
  }
}, JSON.stringify({ account: ACCOUNT_SEED }));

// Mock ALL endpoints the page calls on load
await page.route(/.*securetoken\.googleapis\.com.*/, (route) =>
  route.fulfill({ json: { id_token: 'fake', refresh_token: 'fake', expires_in: '3600' } }));
await page.route(/.*identitytoolkit\.googleapis\.com.*/, (route) =>
  route.fulfill({ json: { users: [{ localId: 'uid', email: 'test@example.com', emailVerified: true }] } }));
await page.route('**/accounts/me', (route) => route.fulfill({ json: ACCOUNT_SEED }));
```

**When to use:** Tests that must not depend on a running backend.

**Rules:**
- `addInitScript` must be called BEFORE `page.goto()` — it runs before page scripts
- EVERY endpoint the page calls on load must be mocked — missing mocks cause connection refused → redirect to sign-in
- Clear Firebase localStorage keys to prevent the SDK from calling `reload()`/`getIdToken()`, which hangs on unmocked Google API calls

### Passkeys (WebAuthn, 1.61+)

For apps with passkey sign-in, use the virtual authenticator. Do not stub `navigator.credentials` by hand.

```ts
test('signs in with a passkey', async ({ context, page }) => {
  await context.credentials.install();            // before navigating to the WebAuthn page
  await context.credentials.create('yourapp.com'); // rpId = effective domain
  await page.goto('/sign-in');
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page).toHaveURL(/\/dashboard/);
});
```

`context.credentials.get()` returns credentials the app registered, including their keys. Persist one and pass it to `create(rpId, { id, userHandle, privateKey, publicKey })` in a later test.

## Firebase auth: what you need to know

1. **Firebase stores tokens in IndexedDB, not localStorage.** By default `storageState` captures localStorage and cookies but not IndexedDB. Pass `storageState({ indexedDB: true })` to include it.
2. **The Pinia auth store in localStorage is what route guards check.** Most Nuxt apps persist auth via `pinia-plugin-persistedstate`. Route middleware reads this store, not IndexedDB.
3. **Pattern B (storageState):** Seed BOTH `firebase:authUser:*` AND your Pinia store key. Missing either: SDK fails to refresh tokens or route guards bounce you.
4. **Pattern C (addInitScript):** Seed ONLY the Pinia store. Clear all `firebase:` keys to prevent the SDK from making network calls that will hang without mocks.

## The `navigateTo()` helper

```ts
async function navigateTo(page: Page, path: string) {
  await page.evaluate((p) => {
    const nuxtApp = (window as any).__nuxt;
    if (nuxtApp?.$router) {
      nuxtApp.$router.push(p);
    } else {
      window.history.pushState({}, '', p);
      window.dispatchEvent(new PopStateEvent('popstate'));
    }
  }, path);
  // A client-side push fires no load event; wait for the route itself.
  await page.waitForURL((url) => url.pathname === new URL(path, url).pathname);
}
```

**Why:** `page.goto('/protected')` on an SSR app triggers a server-side request. SSR middleware can't see IndexedDB tokens → redirect to sign-in. `navigateTo()` uses client-side Vue Router, skipping SSR middleware.

**When you DON'T need it:** `ssr: false` globally → `page.goto()` works everywhere.

**After navigating,** assert on the page content with a web-first assertion such as `await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible()`. Do not add a load-state wait.

**Correct Nuxt 3+ router access:**
- `window.__nuxt.$router` — correct
- `document.querySelector('#__nuxt').__vue_app__.config.globalProperties.$router` — also correct

**DO NOT use:**
- `window.$nuxt` — **Nuxt 2 only.** Does not exist in Nuxt 3+.
- `window.__nuxt_app__` — not a real global.

## Route mock rules

**Rule 1: Origin-agnostic patterns always.**

```ts
// BAD — hardcoded origin (the #1 cause of test failures)
await page.route(/http:\/\/localhost:3001\/api\/users/, ...);

// GOOD — origin-agnostic
await page.route(/.*\/api\/users/, ...);
await page.route('**/api/users', ...);
```

**Rule 2: Check the actual backend URL.** Read `NUXT_PUBLIC_SERVER_URL`, `NUXT_PUBLIC_API_BASE`, or equivalent runtime config. Compare against what mocks intercept.

**Rule 3: Mock Firebase Google API calls** when using addInitScript (Pattern C):
- `securetoken.googleapis.com` — token refresh (getIdToken)
- `identitytoolkit.googleapis.com` — user lookup (reload)

Missing these causes the Firebase SDK to hang, blocking the useApi semaphore.

## Locators and assertions

Prefer role locators and web-first assertions. They retry until the UI settles, so a test needs no load-state waits or sleeps.

| Need | 1.63 idiom |
|---|---|
| Only the visible match (Quasar keeps hidden copies of menus and dialogs mounted) | `page.getByRole('option', { name: 'Admin' }).visible()` |
| Disambiguate same-named controls | `page.getByRole('button', { name: 'Delete', description: 'Removes the draft' })` (accessible description, 1.60+) |
| Element inside an iframe of unknown selector (embedded checkout, OAuth, reCAPTCHA) | `page.frameLocator().getByRole('button', { name: 'Pay' })` searches every frame |
| Page or region structure | `await expect(page.getByRole('main')).toMatchAriaSnapshot(...)`, or `expect(page)` for the whole page (1.60+) |
| Wait for data the page loads | `const res = page.waitForResponse('**/api/posts'); await action; await res;` then assert on the UI |
| File upload drop zone | `await page.getByTestId('dropzone').drop({ files: 'fixtures/avatar.png' })` (1.60+) |
| Visual regression | `await expect(page).toHaveScreenshot('dashboard.webp')` (a `.webp` name captures WebP, 1.62+) |

`toMatchAriaSnapshot` pins structure, not styling. Generate the first snapshot with `npx playwright test --update-snapshots` and review the diff. Keep text patterns loose (`- heading /Welcome/`) where copy changes often.

```ts
await expect(page.getByRole('navigation')).toMatchAriaSnapshot(`
  - link "Dashboard"
  - link "Settings"
  - button "Sign out"
`);
```

## Quasar component selectors

| Component | Issue | Recommended selector |
|---|---|---|
| `<q-btn>` | Renders a real `<button>`, so `getByRole('button', { name })` works; with `to` or `href` it renders a link | `page.getByRole('button', { name: 'Label' })`, or `page.getByRole('link', { name: 'Label' })` when it has `to`/`href` |
| `<q-input>` | Label wrapper around `<input>` | `page.getByLabel('Label')` or `page.getByRole('textbox', { name: 'Label' })` |
| `<q-select>` | Custom `<div role="combobox">`; options teleport to a `q-menu` | `page.getByRole('combobox')`, then `page.getByRole('option', { name }).visible()` |
| `<q-dialog>` | `.q-dialog` wrapper with backdrop | `page.getByRole('dialog')`, or `page.locator('.your-dialog-class')` scoped to your class |
| `<q-banner>` | `.q-banner` wrapper | `page.locator('.q-banner').filter({ hasText: /pattern/i })` |

**Rule:** When a role selector fails on a Quasar component, check the rendered DOM, not the Vue template. Use `npx playwright test --debug` to pick a locator, or read the failing trace (see "Debugging failures").

## Hydration timing

NuxtLink elements before hydration fire full-page navigation (raw `<a>` tag) instead of Vue Router `push`.

```ts
// Wait for hydration (Nuxt 3.4+)
await page.waitForFunction(() => {
  try { return window.useNuxtApp?.().isHydrating === false; }
  catch { return false; }
});

// Alternative: assert on something only the hydrated client renders,
// e.g. data fetched client-side or a <ClientOnly> element.
await expect(page.getByRole('button', { name: 'Open menu' })).toBeEnabled();
```

## Time-dependent UI

Use the clock API for countdowns, session expiry, "posted 5 minutes ago", and scheduled content. Do not use real sleeps or patch `Date` by hand.

```ts
await page.clock.install({ time: new Date('2026-01-15T09:00:00Z') }); // before goto
await page.goto('/billing');
await page.clock.fastForward('30:00');           // jump ahead, fire due timers once
await expect(page.getByText('Session expired')).toBeVisible();
// page.clock.setFixedTime(date) freezes Date.now() for render-only checks
```

## Structure tests with steps

Group each user flow into `test.step` calls. Steps appear in the HTML report, trace viewer and CLI output. `subtitle` and `params` (1.63) label a step without editing its title:

```ts
await test.step('create post', async () => {
  await page.getByRole('button', { name: 'New post' }).click();
  await page.getByLabel('Title').fill(title);
  await page.getByRole('button', { name: 'Publish' }).click();
}, { subtitle: env.name, params: { title } });
```

## Flake control

Configure these in `playwright.config.ts`. The full config is in `references/auth-fixture-template.md`.

- **Test locks (1.63).** Tests that touch a shared resource declare `test('updates profile', { lock: 'test-user-profile' }, ...)`. Examples: the shared test account's settings, a seeded org, a single-tenant sandbox. Tests sharing a lock name never run concurrently, across files and projects. Everything else stays parallel. Prefer this to `test.describe.serial` or `workers: 1`.
- **`retryStrategy: 'isolated'` (1.62).** Retries run at the end, one at a time, so a test that passes alone but fails under load shows up as a flake.
- **`failOnFlakyTests: !!process.env.CI`.** A test that passes only on retry fails CI instead of being ignored.
- **Evidence on failure.** Use `trace: { mode: 'retain-on-failure-and-retries', snapshots: { dom: true, aria: true, screen: true } }` and `video: 'retain-on-failure-and-retries'` (1.61+). Every failing or retried run then keeps a trace with DOM, aria and screen snapshots, plus a video.

## Debugging failures

| Step | Command |
|---|---|
| Extract a failing trace for the CLI (1.59+) | `npx playwright trace open test-results/<test>/trace.zip` |
| List the failed actions | `npx playwright trace actions --errors-only` (or `--grep <title>`) |
| Show one action, its errors and console | `npx playwright trace action <id>`, `trace errors`, `trace console` |
| Inspect the DOM snapshot around an action | `npx playwright trace snapshot <id> --phase before` (`action` / `after`) |
| Step through a test from the terminal (1.62+) | Start `npx playwright test <file> --debug=cli` in the background, then `npx playwright cli attach <session>`; every later command needs `-s=<session>` (see `cas-playwright-debug` §2) |
| Open the trace UI (humans) | `npx playwright show-trace test-results/<test>/trace.zip` |

Read the trace before changing a locator or adding a wait. The trace's aria snapshot shows what the locator could see at the moment it failed.

## Component tests (stories + gallery, 1.62+)

The old `@playwright/experimental-ct-vue` packages are no longer updated. Do not add them to a Nuxt project. Component tests now use the built-in `mount` fixture of `@playwright/test`:

- Write a **story** per scenario. A story is a small wrapper component with hard-coded props, mock data and providers such as Pinia or Quasar.
- Serve a **gallery** page at `baseURL`. The gallery exposes `window.mount(params)` and `window.unmount()`; a Nuxt page or a small Vite app works.
- In tests, call `const c = await mount('components/PostCard/Draft', { title: 'Hi' })`, then scope queries from `c`: `c.getByRole('button')`. Use `c.update(props)` to re-render and `c.unmount()` to remove it.
- Give the component project `use: { reuseContext: true }` for speed. Never set it on E2E projects.

## Diagnostic table

| Symptom | Root cause | Fix |
|---|---|---|
| 500 on protected page (SSR app) | `page.goto()` hit SSR middleware, can't see IndexedDB tokens | Use `navigateTo()` for client-side routing |
| 500 on protected page (SPA app) | Auth store missing from localStorage | Seed Pinia auth store before navigation |
| Redirects to sign-in after storageState | Missing `firebase:authUser:*` or Pinia store key | Seed both keys; wait for persistence before saving storageState |
| `getByRole('button')` times out on a Quasar button | The `<q-btn>` has `to`/`href`, so it renders role `link` | `getByRole('link', { name })` |
| Strict-mode error on a Quasar option or menu item | Hidden teleported copies also match | Add `.visible()` |
| NuxtLink click does nothing | Click fires before hydration | Wait for hydration (see "Hydration timing") |
| Page stuck / never loads | Missing Firebase API mocks (addInitScript pattern) | Mock `securetoken.googleapis.com` and `identitytoolkit.googleapis.com` |
| Tests pass serial, fail parallel | Shared mutable state between workers | Put a `lock` on the tests that share the resource |
| Passes only on retry | Load-dependent flake | `retryStrategy: 'isolated'`; read the retained trace of the failed attempt |
| Route mocks not intercepting | Origin mismatch (mocks `localhost:3001`, app hits Vercel URL) | Use origin-agnostic patterns (`**/api/...`) |
| `window.$nuxt` is undefined | Nuxt 2 API in Nuxt 3+ app | `window.__nuxt.$router` |
| storageState missing tokens | Saved before Firebase/Pinia persistence completed | `expect.poll(() => page.localStorage.getItem('key')).not.toBeNull()` before saving |
| `getByText('Foo')` strict mode error | Multiple elements match | Use a role locator with `name`, add `{ exact: true }`, or scope to a region |
| Assertion flips with the clock | Real time leaks into the UI | `page.clock.install()` before `goto` |

## Retired idioms

When you edit an older suite, replace these:

| Old | Replace with |
|---|---|
| `waitForLoadState('networkidle')` | A web-first assertion on the content, or `page.waitForResponse()` for the request it depends on |
| `locator(':visible')` / `>> visible=true` | `.visible()` |
| `page.evaluate(() => localStorage.setItem(...))` after load | `page.localStorage.setItem()` |
| Hand-rolled `navigator.credentials` stubs | `context.credentials` |
| Synthetic `dispatchEvent('drop')` / hand-built `DataTransfer` for a drop zone | `locator.drop({ files })` |
| `@playwright/experimental-ct-*` | Stories + gallery with the `mount` fixture |
| `test.describe.serial` / `workers: 1` to protect a shared account | `{ lock: 'name' }` on the affected tests |
| `trace: 'on-first-retry'` | `retain-on-failure-and-retries` with aria + screen snapshots |

## Anti-patterns

- **Never `page.goto()` to a protected route on an SSR app** without the `navigateTo()` helper
- **Never use `window.$nuxt`** — Nuxt 2 only, does not exist in Nuxt 3+
- **Never hardcode backend URLs in route mocks** — origin-agnostic patterns always
- **Never skip the Firebase/Pinia persistence wait** when building storageState
- **Never leave Firebase keys in localStorage** when using addInitScript — SDK will hang
- **Never assume `getByRole('button')` works for Quasar buttons** — check rendered DOM first
- **Never wait on load state or `waitForTimeout`** to "let the page settle" — assert on what the user would see

## Running tests

```bash
npx playwright test tests/path/to/test.spec.ts   # Specific file
npx playwright test -g "test name"                 # By name
npx playwright test --debug=cli <file>              # Terminal debugger (agents)
npx playwright test --update-snapshots <file>       # Regenerate aria/visual snapshots, then review
npx playwright show-report                          # HTML report
TEST_ENV=local npx playwright test                  # Switch environment
```
