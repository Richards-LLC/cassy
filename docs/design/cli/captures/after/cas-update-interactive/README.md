# `cas update` interactive captures (native, accepted renderer)

These are native captures, not `--render-fixture` replays: the interactive path prompts, so it
cannot be replayed.

- **Binary:** `cas 3.62.0 (4cedda1 2026-10-17)`, sha256 `9e097923…dcedce`, the debug build that
  passed independent QA round 2 of cas-70a8. Its update renderer is identical to the merged
  `dc1665877`.
- **Driver:** `interactive-proof.py`, saved in the cas-70a8 task artifacts as
  `interactive-proof/round-2/`. It runs the binary in an 80×24 pty (120 columns for the
  `-120` variants) on a disposable scratch host. Every `CAS_*` variable is scrubbed, and HOME
  and cwd are scratch.
- **Release:** a loopback fake release, read through the debug-only
  `CAS_UPDATE_GITHUB_API_URL`. It offers `9.9.9`, or `3.62.0` for `current-80`. Its asset is
  JSON, so an accepted install always fails at extraction and nothing is ever installed. The
  binary's sha was unchanged after every run.
- **Format:** each `.txt` is the terminal stream with escape sequences removed and each
  carriage-return redraw folded to its final state.

| Capture | What it shows |
| --- | --- |
| `accept-80.txt` | prompt, answered `y`, the download on the phase line, then `✗ update failed` on its own line and the three-clause install error |
| `decline-80.txt` | prompt, answered `n`, then `⚠ cancelled · 3.62.0 unchanged` |
| `yes-80.txt` | the same failing asset with `-y`: no prompt, the same error |
| `current-80.txt` | no update available, so no prompt; both phases and the findings of the scratch host's live refresh (one stale `.git/index.lock`) |
| `accept-c-locale.txt` | the accept path under `LC_ALL=C`: ASCII only |
