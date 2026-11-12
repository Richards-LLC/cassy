---
metadata:
  managed_by: cas
---

# Nuxt, Quasar and Firebase

Read nuxt.config and routeRules, app route middleware and auth persistence before selecting fixtures. Global SPA and per-route SSR can have different auth boundaries; server middleware cannot read a browser's IndexedDB tokens. Do not assume one store key or hydration global exists in every Nuxt app.

## Choose the boundary

- Hermetic UI: seed only the app's documented auth contract before scripts run; route every required Firebase/app endpoint through synthetic fixtures and deny other traffic at context scope. Treat token refresh and account lookup as explicit fixture endpoints, not silent exceptions to the allowlist.
- Staging integration: use reserved QA identities and real sign-in or the project's reviewed token adapter. Validate frontend/API origins and remote-mutation permission before provisioning or cleanup. Keep credentials and storageState out of source control and public artifacts.
- API auth does not prove browser auth. Drive the app to its authenticated UI and wait for observable persistence before saving state; a raw REST token or handcrafted Firebase localStorage entry may omit SDK IndexedDB/session state.

## Reuse state, isolate tests

A setup project signs in for each needed role and saves storageState with indexedDB: true for Firebase SDK state. A fresh test-scoped context loads that file for each test. For mutating tests, allocate distinct identities/data per run/shard/worker, or isolate with a documented shared-resource lock within the run. Do not share a worker-scoped page or DOM.

```ts
import { test as setup, expect } from '@playwright/test';

for (const key of ['QA_EMAIL', 'QA_PASSWORD', 'QA_ACCOUNT_ID']) {
  if (!process.env[key]) throw new Error(`Missing required QA input: ${key}`);
}

setup('prepare reserved QA state', async ({ page }) => {
  // Config/fixture has already validated the origins and QA identity.
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(process.env.QA_EMAIL!);
  await page.getByLabel('Password', { exact: true }).fill(process.env.QA_PASSWORD!);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByTestId('account-id')).toHaveText(process.env.QA_ACCOUNT_ID!);
  // Wait through the app's supported persistence/completion seam if asynchronous.
  await page.context().storageState({ path: 'playwright/.auth/qa.json', indexedDB: true });
});
```

Validate those required environment inputs before running setup; absent credentials fail the required lane. The state filename is synthetic local output, ignored by Git. Test IDs and fields above are illustrative app contracts, not Nuxt/Firebase globals to copy blindly.

## Real navigation and hydration

For public or correctly server-authenticated routes, use normal navigation. For an SSR-protected route whose auth exists only in the client, start at an allowed public page, wait for the app's supported client-ready signal, then click its actual NuxtLink/accessible navigation control. Assert both the destination URL and mounted route content. A URL alone does not prove Vue rendered the page.

Banned fallback: history.pushState plus a synthetic popstate event. It can change the address without exercising the router. Avoid guessed window.__nuxt/useNuxtApp globals; use a documented app fixture seam or rendered hydration-dependent control. If direct protected navigation is the product contract, test/fix server-visible auth rather than bypassing it with client routing.

## Requests, data and cleanup

Read runtime API configuration and validate the resolved origin, including redirects. Route by a reviewed origin plus endpoint, not unrestricted origin-agnostic globs. Stub the request before the action and await its response plus the promised UI result. Record created resource IDs; clean only that run's resources and enforce mutation authorization again during teardown. A cleanup permission cannot be inferred from a successful login.

Use [locators-and-accessibility.md](locators-and-accessibility.md) for the single Quasar selector map. Hidden teleported options need semantic .visible() narrowing; link-rendering q-btn needs the link role. Shared-data failures need isolated fixtures or a justified lock, not blanket serialization or retries.

For passkeys, use the supported virtual-authenticator API and the allowed relying-party origin; do not patch navigator.credentials. For time-dependent auth/expiry behavior, install page.clock before app code reads time. Keep real network completion event-driven.

## Stories and gallery

For component-state tests, serve a story gallery at baseURL and expose the documented mount/unmount interface. A story supplies props, fixture data and providers (Pinia/Quasar) for one scenario. Use the built-in mount fixture: const card = await mount('components/PostCard/Draft', { title: fixture.title }); then query card's semantic controls, card.update(props) and card.unmount(). Assert the public outcome; no retired experimental component-testing package is needed. Keep component-story projects distinct from independent journeys and reset their state; a performance optimization must not leak DOM or storage between cases.
