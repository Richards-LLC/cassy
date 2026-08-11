---
name: cas-nuxt-playwright
description: Opt-in only: invoke ONLY when the operator explicitly asks for Nuxt + Playwright E2E testing help; do not invoke for ordinary development, test edits, or routine verification.
managed_by: cas
user-invocable: true
---

# Nuxt + Playwright E2E Testing

Unified guide for writing and debugging Playwright E2E tests against Nuxt (3 & 4) apps with Firebase auth and Quasar UI. Grounded in real failures across production projects.

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

See `references/auth-fixture-template.md` for the ready-to-copy fixture.

**When to use:** Default choice for testing against staging or production. The fixture logs in once via the real sign-in form, then every test in the worker reuses that authenticated page.

**Key properties:**
- Worker-scoped (`{ scope: 'worker' }`) — login happens once, not per test
- Includes `navigateTo()` helper for protected routes on SSR apps
- Includes cleanup tracker for test-created resources
- Multi-environment support via `getEnvConfig()` helper

### Pattern B: storageState from Firebase REST API

Setup project signs in via Firebase REST API, seeds localStorage, saves storageState for dependent test suites.

```ts
// In a setup file (runs before test suites)
const auth = await signInWithFirebaseREST(email, password);

// Seed both Firebase SDK key AND Pinia auth store
await page.evaluate(({ fbKey, fbUser, appAuth }) => {
  localStorage.setItem(fbKey, JSON.stringify(fbUser));
  localStorage.setItem('your-app-auth', JSON.stringify(appAuth));
}, { fbKey: `firebase:authUser:${apiKey}:[DEFAULT]`, fbUser, appAuth });

await page.context().storageState({ path: statePath });
```

**When to use:** Setup projects that pre-authenticate for dependent suites. Faster than UI login but couples to Firebase REST API shape.

**Critical:** Wait for Pinia persistence before saving storageState:
```ts
await page.waitForFunction(
  () => !!localStorage.getItem('your-app-auth'),
  { timeout: 10_000 },
);
```

### Pattern C: addInitScript + full route mocking

Fully isolated tests with no backend dependency.

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
  route.fulfill({ status: 200, contentType: 'application/json',
    body: JSON.stringify({ id_token: 'fake', refresh_token: 'fake', expires_in: '3600' }) }));
await page.route(/.*identitytoolkit\.googleapis\.com.*/, (route) =>
  route.fulfill({ status: 200, contentType: 'application/json',
    body: JSON.stringify({ users: [{ localId: 'uid', email: 'test@test.com', emailVerified: true }] }) }));
await page.route(/.*\/accounts\/me/, (route) => route.fulfill({...}));
```

**When to use:** Tests that must not depend on a running backend.

**Rules:**
- `addInitScript` must be called BEFORE `page.goto()` — it runs before page scripts
- EVERY endpoint the page calls on load must be mocked — missing mocks cause connection refused → redirect to sign-in
- Clear Firebase localStorage keys to prevent the SDK from calling `reload()`/`getIdToken()`, which hangs on unmocked Google API calls

## Firebase auth: what you need to know

1. **Firebase stores tokens in IndexedDB, not localStorage.** `storageState` captures localStorage + cookies but NOT IndexedDB.
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
  await page.waitForLoadState('domcontentloaded');
}
```

**Why:** `page.goto('/protected')` on an SSR app triggers a server-side request. SSR middleware can't see IndexedDB tokens → redirect to sign-in. `navigateTo()` uses client-side Vue Router, skipping SSR middleware.

**When you DON'T need it:** `ssr: false` globally → `page.goto()` works everywhere.

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

## Quasar component selectors

| Component | Issue | Recommended selector |
|---|---|---|
| `<q-btn>` | Nested `<span>` breaks `getByRole('button')` | `page.getByText('Label')` or `page.locator('button', { hasText: 'Label' })` |
| `<q-input>` | Label wrapper around `<input>` | `page.getByLabel('Label')` or `page.getByRole('textbox', { name: 'Label' })` |
| `<q-select>` | Custom `<div role="combobox">` | `page.getByRole('combobox')` |
| `<q-dialog>` | `.q-dialog` wrapper with backdrop | `page.locator('.your-dialog-class')` — scope to your class |
| `<q-banner>` | `.q-banner` wrapper | `page.locator('.q-banner').filter({ hasText: /pattern/i })` |

**Rule:** When a role selector fails on a Quasar component, check the rendered DOM (`page.locator('...').count()` to debug), not the Vue template.

## Hydration timing

NuxtLink elements before hydration fire full-page navigation (raw `<a>` tag) instead of Vue Router `push`.

```ts
// Wait for hydration (Nuxt 3.4+)
await page.waitForFunction(() => {
  try { return window.useNuxtApp?.().isHydrating === false; }
  catch { return false; }
});

// Pragmatic alternative
await page.waitForLoadState('networkidle');
await page.locator('.your-element').waitFor({ state: 'visible' });
```

## Diagnostic table

| Symptom | Root cause | Fix |
|---|---|---|
| 500 on protected page (SSR app) | `page.goto()` hit SSR middleware, can't see IndexedDB tokens | Use `navigateTo()` for client-side routing |
| 500 on protected page (SPA app) | Auth store missing from localStorage | Seed Pinia auth store before navigation |
| Redirects to sign-in after storageState | Missing `firebase:authUser:*` or Pinia store key | Seed both keys; wait for persistence before saving storageState |
| `getByRole('button')` times out | Quasar `<q-btn>` nested spans | `getByText()` or `locator('button', { hasText })` |
| NuxtLink click does nothing | Click fires before hydration | Wait for `networkidle` + element visibility |
| Page stuck / never loads | Missing Firebase API mocks (addInitScript pattern) | Mock `securetoken.googleapis.com` and `identitytoolkit.googleapis.com` |
| Tests pass serial, fail parallel | Shared mutable state between workers | `fullyParallel: false` or `test.describe.serial` |
| Route mocks not intercepting | Origin mismatch (mocks `localhost:3001`, app hits Vercel URL) | Use origin-agnostic patterns (`**/api/...`) |
| `window.$nuxt` is undefined | Nuxt 2 API in Nuxt 3+ app | `window.__nuxt.$router` |
| storageState missing tokens | Saved before Firebase/Pinia persistence completed | `waitForFunction(() => !!localStorage.getItem('key'))` before saving |
| `getByText('Foo')` strict mode error | Multiple elements match | Add `{ exact: true }` or use `.first()` |

## Anti-patterns

- **Never `page.goto()` to a protected route on an SSR app** without the `navigateTo()` helper
- **Never use `window.$nuxt`** — Nuxt 2 only, does not exist in Nuxt 3+
- **Never hardcode backend URLs in route mocks** — origin-agnostic patterns always
- **Never skip the Firebase/Pinia persistence wait** when building storageState
- **Never leave Firebase keys in localStorage** when using addInitScript — SDK will hang
- **Never assume `getByRole('button')` works for Quasar buttons** — check rendered DOM first

## Running tests

```bash
npx playwright test tests/path/to/test.spec.ts   # Specific file
npx playwright test -g "test name"                 # By name
npx playwright test --workers=1                     # Debug race conditions
npx playwright show-report                          # HTML report
TEST_ENV=local npx playwright test                  # Switch environment
```
