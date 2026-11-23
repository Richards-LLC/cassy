### Fixed — Worker check

- **A Playwright test declared with the tag option can now earn its stability proof** — Was: the capped Playwright stability run selected a test by a title pattern that had to end the line, but Playwright greps the title followed by the test's tags, so a test such as `test('t', { tag: ['@staging'] }, fn)` was never found and could not be parked ("No tests found"). Now: the pattern allows trailing `@tag` words after the title, so tagged and untagged tests are both selected and a different test whose title merely starts the same is still left out (GH #2349).
