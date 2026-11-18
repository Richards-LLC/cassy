### Fixed — Worker check

- **Jest specs run under the config that owns them** — Was: a changed spec outside the nearest `jest.config.*` (such as an integration spec with its own config) ran under that config, found no tests and could not earn stability. Now: worker-check and the stability run pick the Jest config whose `roots`, `projects` and `testRegex`/`testMatch` own the spec, so the prescribed command runs and counts its tests.
