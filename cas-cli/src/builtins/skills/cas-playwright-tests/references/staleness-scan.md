---
metadata:
  managed_by: cas
---

# Static staleness scan

Before you run, rewrite or trust a large suite, check every spec against the current app sources. Running the suite and triaging the red misses the worst case: specs that pass while testing nothing. Examples: a `toHaveCount(0)` that waits on a class that no longer exists, a `test.skip` when a renamed button is missing, or a helper that reads a retired API path. The scan needs no browser, no target and no model spend, and it can run per area in parallel.

## Run it

```bash
python3 -I <skill-dir>/scripts/staleness-scan.py --root <project> \
  --specs 'e2e/**/*.spec.ts' \
  --routes nuxt:web/pages --api nestjs:api/src --api-prefix '^/api/' \
  --sources web > <task-artifacts>/staleness.md
```

The scan reads each spec and the relative helpers it imports. It does not read app sources that a spec imports. From those files it extracts four kinds of dependency:

- routes passed to `goto`, `toHaveURL` and `waitForURL` (one that matches `--api-prefix` counts as an API path)
- string literals that match `--api-prefix`
- test ids from `getByTestId` and `data-testid`
- classes and `data-*` attributes in `locator()` selectors

It resolves each dependency against the current sources. An exact path must match one route; only a glob or template tail (`/v1/**`, `` `/v1/${id}` ``) or a `startsWith` argument may match the start of a route. A root catch-all (an SPA fallback, a CORS preflight or a Nuxt `[...slug]` page) is ignored, because it would match every path. Test and e2e files are never treated as app sources. The default Markdown output lists every dependency with its spec line and its source receipt; `--json` emits the same data. Omitting `--routes`, `--api` or `--sources` leaves those dependencies unchecked; an index that is given but empty reports them missing.

## Grades

| Grade | Meaning | Suggested action |
| --- | --- | --- |
| CURRENT | Every dependency resolves to a cited source line | keep |
| UNSURE | Nothing is missing, but a test id or `data-*` attribute is only built at runtime | grep the setter, then grade |
| UNCHECKED | Nothing is missing, but an index was omitted | rerun with the missing index |
| STALE | At least one dependency is missing | rewrite |
| DEAD | Dependencies are missing and none resolve | delete, or rewrite from the current app |

The script grades; it does not decide. Read every spec that is not CURRENT and settle its grade by citing the current source line. Then confirm the suggested `kind` (journey, regression or one-off) and choose the action: keep, rewrite, delete, or fold it into the journey that already covers it. Record the table and your decisions in the task artifacts before any rewrite.

Static search misses attributes that library code sets at runtime (for example a popover's `dataset.open = 'true'`). A `data-*` selector it cannot find is UNSURE, never missing. Test ids built from templates (`` `syntax-${kind}` ``) or from field names (`field.replaceAll('_', '-')`) are also UNSURE. The finding names a candidate source line to read first.

## Adapters and extension points

| Flag | Adapter | Resolves |
| --- | --- | --- |
| `--routes` | `nuxt:<pages>` | `[param]`, `[[optional]]`, `[...rest]`, `(group)`, `index`, plus `routeRules` redirects in the sibling `nuxt.config` |
| `--routes` | `static:<root>` | Vite or static roots: each `index.html` serves its directory |
| `--api`, `--routes` | `axum:<dir>` | Rust `.route("/v1/x/{id}", …)`, including multi-line calls; `.nest()` prefixes are not joined |
| `--api` | `nestjs:<dir>` | `@Controller` joined with `@Get`/`@Post`/…, `:param`, and `setGlobalPrefix` |
| `--api`, `--routes` | `list:<file>` | one pattern per line, for routes that server middleware or another service defines |

To support another framework, add a function to `ROUTE_ADAPTERS` or `API_ADAPTERS`. It takes `(root, path)` and returns `[(pattern, file, line)]`. Patterns use `:p`, `{p}` or `[p]` for one segment, and `*`, `**`, `{*p}` or `[...p]` for the rest of the path. Redirects in route middleware or server middleware are code, so list their source paths with `list:`. Do not let them silently resolve everything.

<!-- cas-src:begin -->
## Worked example: Commander

Run from a Cassy checkout. It takes about a second and needs no browser:

```bash
python3 -I cas-cli/src/builtins/skills/cas-playwright-tests/scripts/staleness-scan.py --root . \
  --specs 'hub-web/e2e/**/*.spec.ts' --specs 'hub-web/e2e/**/*.journey.ts' \
  --routes static:hub-web --api axum:cas-cli/src/hub --api-prefix '^/v\d+/' --sources hub-web/src
```

On November 23, 2026 the scan graded 50 specs: 47 CURRENT, 3 UNSURE, 0 STALE and 0 DEAD. It checked 1,792 dependencies: 1,020 hub API paths, 401 test ids, 188 classes, 115 routes and 68 `data-*` attributes.

- Every hub API path that the journeys and the hub double use resolved to a `.route(…)` line in `cas-cli/src/hub/server.rs`.
- The three UNSURE specs depend on runtime-built test ids. `credential-generation` and `key-fingerprint` come from `installation-inventory.ts`, which sets `testid` from the field name. `syntax-keyword`, `syntax-comment` and `syntax-string` come from `rich-syntax.ts`, which sets `` `syntax-${kind}` ``. Reading those setters settles all three as CURRENT.
<!-- cas-src:else -->
## Worked example: a Vite app on a Rust hub

```bash
python3 -I <skill-dir>/scripts/staleness-scan.py --root . \
  --specs 'web/e2e/**/*.journey.ts' \
  --routes static:web --api axum:server/src --api-prefix '^/v\d+/' --sources web/src
```

On a 50-spec journey suite the scan checked 1,792 dependencies in about a second: 47 CURRENT, 3 UNSURE, 0 STALE, 0 DEAD. All three UNSURE specs used test ids built at runtime (`` `syntax-${kind}` ``, or a field name with `_` replaced by `-`). Reading the setter named in each finding settled them as CURRENT.
<!-- cas-src:end -->
