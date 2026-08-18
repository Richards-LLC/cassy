---
from: Ozer Health team
date: 2026-05-26
priority: P2
---

# Feature Request: Generic Nuxt 3 + Playwright E2E Skill

## What we need

A **framework-level Cassy skill** for writing Playwright E2E tests against Nuxt 3 applications. We built a project-specific version at `.claude/skills/cas-nuxt-playwright/SKILL.md` in the Ozer repo after burning hours on patterns that should be documented once and reused everywhere.

## Why

Every Nuxt 3 project will hit the same issues:
- SSR vs SPA auth middleware confusion (which pages are SSR, which aren't)
- Firebase/Supabase/Auth0 token persistence timing (IndexedDB vs localStorage vs cookies)
- Quasar/Vuetify/PrimeVue component selectors that break `getByRole`
- NuxtLink hydration race conditions
- `storageState` not capturing auth tokens stored in IndexedDB
- `page.goto()` vs SPA navigation patterns

These are framework-level patterns, not project-specific. A generic skill would prevent every Cassy-managed Nuxt project from rediscovering them.

## Proposed scope

A `cas-nuxt-playwright` skill that covers:

### 1. SSR detection and routing
- How to determine if a Nuxt app uses `ssr: true`, `ssr: false`, or per-route `routeRules`
- Decision tree: when `page.goto()` is safe vs when SPA navigation is required
- How SSR middleware differs from client-side middleware in test context

### 2. Auth patterns (framework-agnostic)
- **Cookie-based auth** (SSR-compatible): `useCookie()` pattern, storageState preserves cookies
- **localStorage-based auth** (SPA-only): Pinia persist, `addInitScript` seeding
- **IndexedDB-based auth** (Firebase, Supabase): storageState does NOT capture IndexedDB; must seed via `addInitScript` or sign in through the UI
- Decision table: auth provider → which pattern to use

### 3. Hydration safety
- `window.useNuxtApp().isHydrating` check (Nuxt 3.4+)
- `document.querySelector('#__nuxt').__vue_app__` for router access
- `data-hydrated` attribute pattern for framework-agnostic hydration detection
- When to wait for hydration vs when it doesn't matter

### 4. Component library selectors
- Quasar: `<q-btn>` renders nested spans, `getByRole('button')` may not work → `getByText()` fallback
- General pattern: check the component library's rendered DOM, not the Vue template
- Table of common components → recommended Playwright selectors

### 5. Programmatic navigation
- Nuxt 3 router access: `document.querySelector('#__nuxt').__vue_app__.config.globalProperties.$router`
- NOT `window.$nuxt` (Nuxt 2 only)
- NOT `window.__nuxt_app__` (not a real global)
- `page.evaluate` patterns for `router.push()`

### 6. Common failure modes
- Diagnostic table: symptom → root cause → fix
- Covers: 500 on protected pages, NuxtLink click does nothing, storageState missing tokens, parallel spec interference, Stripe iframe handling

## What we already built (reference)

Our project-specific skill is at:
```
~/Petrastella/ozer/.claude/skills/cas-nuxt-playwright/SKILL.md
```

It's Ozer-specific (references our test accounts, staging URLs, specific selectors) but the patterns are generic. The Cassy skill should extract the framework-level knowledge and make it reusable across projects.

## Lessons learned the hard way (2026-05-26)

1. **`ssr: false` means NO SSR.** We spent 2+ hours debugging "SSR middleware" that didn't exist. The 500s were client-side error boundaries, not server-side renders.
2. **Firebase uses IndexedDB, not localStorage.** `storageState` only captures localStorage + cookies. Firebase auth tokens are invisible to Playwright's storageState. But the Pinia auth store (`ozer-auth`) in localStorage was sufficient — the Firebase SDK handles its own token refresh via IndexedDB.
3. **`window.$nuxt` is Nuxt 2.** Every AI agent (including us) tries this first. It doesn't exist in Nuxt 3.
4. **Quasar `<q-btn>` breaks `getByRole('button')`.** The nested `<span>` structure confuses Playwright's accessibility tree. `getByText()` is the reliable fallback.
5. **NuxtLink clicks before hydration fire full-page navigation.** The raw `<a>` tag does a browser navigation instead of Vue Router push. Wait for `networkidle` + element visibility.

## Delivery preference

A Cassy skill file at `~/.claude/skills/cas-nuxt-playwright/SKILL.md` (global, not project-scoped) that any Nuxt 3 project can reference. Should be auto-invoked when:
- Editing or creating files under `tests/` in a Nuxt 3 project
- Debugging Playwright test failures in a Nuxt 3 project
- The `cas-playwright-debug` skill detects a Nuxt project (presence of `nuxt.config.ts`)
