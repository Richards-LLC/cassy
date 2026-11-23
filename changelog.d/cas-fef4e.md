### Changed — Testing skills

- **Playwright authoring covers async jobs, personas and paid lanes** — Was:
  the Playwright authoring skill had one shared sign-in state, opt-out live
  smoke and no guidance for background jobs or `.mjs` helpers. Now: it grades
  background jobs on server status, routes specs to per-persona signed-in
  projects by tag, gates paid and staging-only tests behind env vars in config,
  and loads helper `.mjs` as native ESM via `build.external`.
