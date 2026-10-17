# `cas update` interactive captures (native, accepted renderer)

These are native captures from the independently approved QA run, not `--render-fixture`
replays: the interactive path prompts, so it cannot be replayed.

- **Binary:** a private copy of the root-built `cas 3.62.0 (4cedda1 2026-10-17)`, sha256
  `9e097923384730f41b925c04c08660aa5ceee5b48b4fee335b540f63b3dcedce`
  (`candidate-version.txt`). Its update renderer is identical to the merged `dc1665877`.
- **Who ran it:** the factory supervisor, through the QA reviewer's driver (`run-root.sh`,
  `run-root-color.sh`, `accept-proof.py`; copied here unchanged), during independent QA round 2
  of cas-70a8. That round approved this build.
- **Environment:** factory identity and `CAS_CLONE_PATH` were preserved exactly as the supervisor
  had them; each cell's `.json` records them under `identity_preserved`. Each run used a fresh
  physical scratch HOME, cwd, store and project roots. The driver itself refuses a worker role.
- **Release:** a loopback fake release, read through the debug-only `CAS_UPDATE_GITHUB_API_URL`,
  offers `9.9.9`. The `garbage` asset downloads and then fails extraction, `slow` downloads
  slowly before failing, and `500` returns HTTP 500. Nothing is ever installed, and each
  `.json` records `binary_unchanged: true` and the requests made.
- **Format:** each `.txt` is the terminal stream with escape sequences removed and redraws
  folded. The three `-color` cells also keep their raw `.ansi` stream.

| Cell | What it shows |
| --- | --- |
| `cand-y-garbage-80`, `-120` | prompt answered `y`, the download on the phase line, `update failed` on its own line, then the three-clause install error; widest 80 at both widths |
| `cand-y-slow-80`, `cand-y-slow-120-color` | the same with a slow download, so the live `downloading Cassy 9.9.9` line redraws before the result replaces it |
| `cand-y-500-80` | the download returns HTTP 500: `Download request failed with status: 500`, binary unchanged |
| `cand-n-80`, `cand-n-80-color` | prompt answered `n`: `cancelled · 3.62.0 unchanged`, and `/asset` is never requested |
| `cand-yes-garbage-80` | `-y`: no prompt, the same error |
| `cand-pipe-garbage` | piped stdout: a start line and a result line, with the error on stderr |
| `cand-y-garbage-C`, `cand-y-garbage-nocolor` | `LC_ALL=C` and `NO_COLOR`: bracket marks, and no colour sequences under `NO_COLOR` |

The approved round did not include an up-to-date (no-prompt) interactive cell with preserved
identity, so none is published here. The `--render-fixture` captures beside this directory
show the up-to-date result and receipt.
