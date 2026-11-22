### Fixed — Worker test stability

- **A Node test that launches Playwright gets a Node rerun command** — When park asks for five passes of a changed test, it picks the runner from the modules the file actually imports. A `node:test` file whose fixture writes or launches a Playwright spec is now told to rerun with `node --test`, not Playwright. A real Playwright spec is still told to use Playwright.
