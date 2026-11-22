### Fixed — Task close

- **A slow moment no longer refuses every close from a healthy checkout** — When one close's read of its own checkout timed out under load, Cassy remembered that failure for 30 seconds. Every close from the same healthy checkout in that window, such as the rest of a batch, was then refused with G-38 "work target repository unavailable" or "mismatch". Now each close re-reads its own checkout and gets the full lookup budget for it. Unrelated registered roots that hang are still skipped (GH #1941, GH #2178).
