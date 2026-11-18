### Fixed — Worker check

- **Jest specs run under the config that selects them** — Was: a changed spec outside the nearest `jest.config.*` testRegex (such as an integration spec with its own config) ran under that config, found no tests and could not earn stability. Now: worker-check picks the Jest config in the spec's directories whose `testRegex`/`testMatch` selects it, so the prescribed command runs and counts its tests.
