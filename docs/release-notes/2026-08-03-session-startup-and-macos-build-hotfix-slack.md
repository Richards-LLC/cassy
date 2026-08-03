# 2026-08-03 — v2.38.2 session startup + macOS build hotfix — #cas-internal posts

## Post 1 — User

**Live on production — User** (v2.38.2)

Was: starting a session left you on the splash screen forever — no error, no timeout, just a boot that never finished. Now: sessions start and come up normally.

- If you have been stuck watching a splash screen for the last few days, this is why. Nothing you did caused it and nothing was lost — the background process that runs your session was dying the instant it started, and the screen you were looking at was patiently waiting for something that was never coming.
- On Macs, the project also would not build at all, so Mac users could not update to pick up recent work. That is fixed in the same release — update and build normally.
- No action needed beyond updating. Existing sessions and stored context are untouched.

## Post 2 — Dev

**Live on production — Dev** (v2.38.2)

Was: every background-process entry point opened its trace log with `O_APPEND` and `O_TRUNC` in a single `open()`. The standard library rejects that pair with `EINVAL` ("creating or truncating a file requires write or append access") — a check that lives in the shared Unix layer, so it failed on Linux and macOS alike. Now: the truncate and the append descriptor are two separate opens, and startup proceeds.

- The failure was invisible by construction. In the forked path it happened after `setsid()` and after stdout/stderr had been redirected, so the child died with nothing on any console; the parent returned normally and attached as a client to a socket that no longer had anyone serving it. The result was an indefinite hang rather than a crash or an error message.
- `O_APPEND` on the returned descriptor is deliberate and preserved: a second, independent descriptor appends audit records to the same file, and a plain write descriptor would start at offset 0 and overwrite them. Getting both properties requires two opens — that is the whole fix.
- Regression coverage added for all three properties: the rejected flag combination, append semantics against a concurrent second descriptor, and parent-directory creation.
- Separately in this release: filesystem capacity math multiplied `statvfs` block counts by fragment size without widening first. Linux types those counts as `u64` so it compiled; macOS types them as `u32` while fragment size stays `u64`, so the build failed outright on Darwin. All fields now widen to `u64` before the multiply, which is a no-op on Linux.
- Both defects shared a root cause worth noting: neither could be caught by the existing checks, because nothing compiled the project on macOS and nothing exercised background-process startup. The flag-combination bug shipped and sat undetected for three days.
