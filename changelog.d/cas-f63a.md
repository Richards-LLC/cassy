### Fixed — Worker check

- **A Vue component no longer demands a proof that cannot run** — Was: park asked for `vitest related --run <file>.vue` even when the project's vitest has no Vue plugin, so the command crashed while transforming the component and no receipt, or override, could ever close the delivery. Now: a component that vitest cannot load proves through the specs that import it, or through its package's own vitest run, and park only prescribes commands that can run (GH #2352).
