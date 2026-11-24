### Fixed — Capped Jest runs from the package directory

- **Capped Jest stability runs start in the package that owns the Jest config** — Before this fix, a spec owned by a config kept in `test/` ran from `test/`. Fixture paths relative to the package, such as `prisma/migrations/.../migration.sql`, then failed with ENOENT before any assertion ran, even though the same raw `jest --config test/...` command passed from the package directory. Worker-check now runs Jest from the nearest ancestor of the config that holds a `package.json`, whatever the config's `rootDir`.
