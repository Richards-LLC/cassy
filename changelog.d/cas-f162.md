### Fixed — Release

- **Release closeout closes every issue, or names the ones it could not** — Was: closeout built one 6-second deadline for the whole GitHub pass, so one slow call timed out every later `gh issue close` (22 of 22 on v4.22, 5 of 6 on v4.21) and the issues stayed open until someone closed them by hand. Now: each close gets its own 30-second deadline and up to three attempts with a growing pause, and an issue that still fails is listed by number in the closeout notice, stays open, so the next closeout pass reads it as open again.
