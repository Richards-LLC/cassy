### Fixed — Factory worker checks

- **Playwright specs run under the config that lists them** — When a directory holds several Playwright configs, the capped stability run and the command park prescribes now use the config that lists the changed spec, such as `playwright.hermetic.config.ts` or `playwright.real-hub.config.ts`. Before, they used the default config, which found no tests, so the spec could never earn its five-pass proof. A `--config` the caller passes still wins, and a run that executes no tests is still not proof (GH #2195).
