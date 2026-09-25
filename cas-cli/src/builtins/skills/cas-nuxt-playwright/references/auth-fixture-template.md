# Auth Fixture Template

Ready-to-copy Playwright fixture for Nuxt (3 & 4) apps with Firebase auth, written for `@playwright/test` 1.63. Modeled after a production Nuxt test suite.

## tests/config/environments.ts

```ts
export type Environment = 'local' | 'staging' | 'prod';

export interface EnvUser {
  email: string;
  password: string;
  displayName: string;
}

export interface EnvConfig {
  name: Environment;
  frontendUrl: string;
  backendUrl: string;
  testUser: EnvUser;
  adminUser?: EnvUser;
  allowMutations: boolean;
}

const environments: Record<Environment, EnvConfig> = {
  local: {
    name: 'local',
    frontendUrl: 'http://localhost:3000',
    backendUrl: 'http://localhost:3001',
    testUser: {
      email: 'playwright@yourapp.test',
      password: 'PlaywrightTest123!',
      displayName: 'Playwright Tester',
    },
    allowMutations: true,
  },
  staging: {
    name: 'staging',
    frontendUrl: 'https://staging.yourapp.com',
    backendUrl: 'https://staging-api.yourapp.com',
    testUser: {
      email: 'playwright@yourapp.test',
      password: 'PlaywrightTest123!',
      displayName: 'Playwright Tester',
    },
    allowMutations: true,
  },
  prod: {
    name: 'prod',
    frontendUrl: 'https://yourapp.com',
    backendUrl: 'https://api.yourapp.com',
    testUser: {
      email: 'playwright@yourapp.test',
      password: 'PlaywrightTest123!',
      displayName: 'Playwright Tester',
    },
    allowMutations: false,
  },
};

export function getEnvConfig(): EnvConfig {
  const env = (process.env.TEST_ENV || 'staging') as Environment;
  const config = environments[env];
  if (!config) {
    throw new Error(
      `Unknown TEST_ENV="${env}". Valid: ${Object.keys(environments).join(', ')}`,
    );
  }
  return config;
}
```

## tests/config/auth-fixture.ts

```ts
import {
  test as base,
  expect,
  request as playwrightRequest,
  type Page,
} from '@playwright/test';
import { getEnvConfig } from './environments';

const env = getEnvConfig();

/**
 * Navigate via client-side Vue Router. Required for protected routes on
 * SSR apps where server-side middleware can't see Firebase IndexedDB tokens.
 * Safe to use on SPA apps too (just a no-overhead client-side push).
 */
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

/**
 * Wait until Nuxt finishes hydrating. Before that, form inputs are not bound
 * to Vue state and NuxtLink clicks trigger full-page loads.
 */
async function waitForHydration(page: Page) {
  await page.waitForFunction(() => {
    try { return (window as any).useNuxtApp?.().isHydrating === false; }
    catch { return false; }
  });
}

/** Real login via the sign-in form — adapt labels to your app. */
async function signIn(page: Page, user: { email: string; password: string }) {
  await page.goto('/sign-in');
  await waitForHydration(page);
  await page.getByLabel('Email').fill(user.email);
  await page.getByLabel('Password').fill(user.password);
  await page.getByRole('button', { name: /^sign in$/i }).click();
  await page.waitForURL((url) => !url.pathname.includes('/sign-in'), {
    timeout: 20_000,
  });
}

export const test = base.extend<
  { navigateTo: (path: string) => Promise<void> },
  { authedPage: Page }
>({
  authedPage: [
    async ({ browser }, use) => {
      // Worker-scoped fixtures do not inherit `use` options such as
      // baseURL, so pass them to the context explicitly.
      const context = await browser.newContext({ baseURL: env.frontendUrl });
      const page = await context.newPage();
      await signIn(page, env.testUser);

      await use(page);
      await context.close();
    },
    { scope: 'worker' },
  ],

  navigateTo: async ({ authedPage: page }, use) => {
    await use((path: string) => navigateTo(page, path));
  },
});

export { expect, env, navigateTo, signIn, waitForHydration };
```

## Usage in test files

```ts
// tests/my-feature.spec.ts
import { test, expect, env } from './config/auth-fixture';

test.describe('My feature', () => {
  test('can navigate to protected page', async ({ authedPage: page, navigateTo }) => {
    // Use navigateTo for protected routes (required on SSR apps)
    await navigateTo('/dashboard');

    // Web-first assertion: retries until the page renders, no load-state wait.
    await expect(page.getByRole('heading', { name: 'Dashboard' })).toBeVisible();
  });

  // Every test in this worker shares one account. Tests that change its state
  // declare a lock so they never run concurrently with each other.
  test('updates display name', { lock: 'test-user-profile' }, async ({ authedPage: page, navigateTo }) => {
    await navigateTo('/settings/profile');

    await test.step('rename', async () => {
      await page.getByLabel('Display name').fill('Renamed Tester');
      const saved = page.waitForResponse('**/api/accounts/me');
      await page.getByRole('button', { name: 'Save' }).click();
      await saved;
    }, { subtitle: env.name });

    await expect(page.getByRole('main')).toMatchAriaSnapshot(`
      - heading "Profile"
      - textbox "Display name": Renamed Tester
    `);
  });

  test('unauthenticated page works with page.goto', async ({ page }) => {
    // Public pages — use the default unauthenticated page fixture
    await page.goto('/');
    await expect(page.getByText('Welcome')).toBeVisible();
  });
});
```

## playwright.config.ts

```ts
import { defineConfig, devices } from '@playwright/test';
import { getEnvConfig } from './tests/config/environments';

const env = getEnvConfig();

export default defineConfig({
  testDir: './tests',
  fullyParallel: false, // Tests share auth context per worker
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  retryStrategy: 'isolated',             // 1.62: retries run last, one at a time
  failOnFlakyTests: !!process.env.CI,    // a pass-on-retry fails CI
  workers: process.env.CI ? 2 : 4,       // shared-account tests use `lock`, not workers: 1
  reporter: 'html',
  timeout: 30_000,
  use: {
    baseURL: env.frontendUrl,
    trace: {
      mode: 'retain-on-failure-and-retries',
      snapshots: { dom: true, aria: true, screen: true }, // 1.63
    },
    screenshot: 'only-on-failure',
    video: 'retain-on-failure-and-retries', // 1.61
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        // System Chrome fallback for hosts without bundled Chromium
        ...(process.env.PLAYWRIGHT_USE_SYSTEM_CHROME === '1'
          ? { channel: 'chrome' }
          : {}),
      },
    },
  ],
  // Auto-start dev server when running against local
  ...(env.name === 'local' ? {
    webServer: {
      command: 'pnpm dev',
      url: 'http://localhost:3000',
      reuseExistingServer: true,
      timeout: 120_000,
    },
  } : {}),
});
```

## Adding test resource cleanup

If tests create resources (posts, users, etc.) that should be cleaned up:

```ts
const trackedResourceIds = new Set<string>();

export function registerTestResource(id: string): void {
  trackedResourceIds.add(id);
}

// In the authedPage fixture, add cleanup in the teardown:
// After `await use(page);`:
if (trackedResourceIds.size > 0) {
  const cleanupRequest = await playwrightRequest.newContext();
  try {
    for (const id of trackedResourceIds) {
      await cleanupRequest.delete(`${env.backendUrl}/api/resources/${id}`);
    }
  } finally {
    await cleanupRequest.dispose();
    trackedResourceIds.clear();
  }
}
```

## Adding admin fixture

For tests that need admin/super-admin access, create a parallel fixture:

```ts
// tests/config/admin-fixture.ts — same pattern, uses env.adminUser
import { test as base, type Page } from '@playwright/test';
import { env, navigateTo, signIn } from './auth-fixture';

export const test = base.extend<
  { navigateTo: (path: string) => Promise<void> },
  { adminPage: Page }
>({
  adminPage: [
    async ({ browser }, use) => {
      if (!env.adminUser) {
        throw new Error(`Environment "${env.name}" has no adminUser configured`);
      }
      const context = await browser.newContext({ baseURL: env.frontendUrl });
      const page = await context.newPage();
      await signIn(page, env.adminUser);
      await use(page);
      await context.close();
    },
    { scope: 'worker' },
  ],
  navigateTo: async ({ adminPage: page }, use) => {
    await use((path: string) => navigateTo(page, path));
  },
});
```
